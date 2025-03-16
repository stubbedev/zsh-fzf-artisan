#--------------------------------------------------------------------------
# Laravel artisan plugin for zsh with fzf integration
#--------------------------------------------------------------------------
#
# This plugin adds an `artisan` shell command that will find and execute
# Laravel's artisan command from anywhere within the project. It also
# adds shell completions that work anywhere artisan can be located.

# Cache setup
ARTISAN_CACHE_DIR="${HOME}/.cache/artisan"
mkdir -p "$ARTISAN_CACHE_DIR"

function artisan() {
  local artisan_path=$(_artisan_find)

  if [ "$artisan_path" = "" ]; then
    >&2 echo "zsh-artisan: artisan not found. Are you in a Laravel directory?"
    return 1
  fi

  local artisan_cmd="php $artisan_path"

  local artisan_start_time=$(date +%s)

  eval $artisan_cmd $*

  local artisan_exit_status=$? # Store the exit status so we can return it later

  if [[ $1 = "make:"* && $ARTISAN_OPEN_ON_MAKE_EDITOR != "" ]]; then
    # Find and open files created by artisan
    find \
      "$(dirname $artisan_path)/app" \
      "$(dirname $artisan_path)/tests" \
      "$(dirname $artisan_path)/database" \
      -type f \
      -newermt "-$(($(date +%s) - $artisan_start_time + 1)) seconds" \
      -exec $ARTISAN_OPEN_ON_MAKE_EDITOR {} \; 2>/dev/null
  fi

  return $artisan_exit_status
}

compdef _artisan artisan

function _artisan_find() {
  # Look for artisan up the file tree until the root directory
  local dir=.
  until [ $dir -ef / ]; do
    if [ -f "$dir/artisan" ]; then
      echo "$dir/artisan"
      return 0
    fi
    dir+=/..
  done
  return 1
}

function _artisan() {
  local state
  local artisan_path=$(_artisan_find)

  _arguments \
    '1: :->command' \
    '*: :->args'

  case $state in
  command)
    if [[ -n "$artisan_path" ]]; then
      local project_dir=$(dirname "$artisan_path")
      local cache_key=$(echo "$project_dir" | md5sum | cut -d' ' -f1).cache
      local cache_file="${ARTISAN_CACHE_DIR}/${cache_key}"
      local composer_lock="${project_dir}/composer.lock"
      local current_command_list="${ARTISAN_CACHE_DIR}/${cache_key}.current"
      local artisan_cmd="php $artisan_path"

      # Generate current command list
      eval "$artisan_cmd list --format=json" | jq -r '.commands[] | "\(.name)\t\(.description)"' >"$current_command_list"

      # Cache invalidation check
      if [[ ! -f "$cache_file" || "$artisan_path" -nt "$cache_file" ||
        (-f "$composer_lock" && "$composer_lock" -nt "$cache_file") ]]; then
        mv -f "$current_command_list" "$cache_file"
      elif ! cmp -s "$current_command_list" "$cache_file"; then
        mv -f "$current_command_list" "$cache_file"
      else
        rm -f "$current_command_list"
      fi

      # Adjusted fzf command to display descriptions
      cat "$cache_file" | fzf --height=40% --reverse --prompt="Artisan Command > " --preview 'echo {}' --bind 'tab:accept' --preview-window=right:50% | awk -F"\t" '{print $1}' | read -r line
      ret=$?
      if [ -n "$line" ]; then
        compadd -U -- "$line"
      fi
      return $ret
    fi
    ;;
  *)
    _files
    ;;
  esac
}
