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
      if [[ "$OSTYPE" == "darwin"* ]]; then
        local cache_key=$(echo "$project_dir" | md5 | awk '{print $1}').cache
      else
        local cache_key=$(echo "$project_dir" | md5sum | cut -d' ' -f1).cache
      fi
      local cache_file="${ARTISAN_CACHE_DIR}/${cache_key}"
      local composer_lock="${project_dir}/composer.lock"
      local current_command_list="${ARTISAN_CACHE_DIR}/${cache_key}.current"
      local artisan_cmd="php $artisan_path"

      # Check if jq is installed
      if ! command -v jq &>/dev/null; then
        echo "jq is not installed. Please install it to use this script."
        echo "On macOS, you can install it using Homebrew: brew install jq"
        echo "On Linux, you can install it using your package manager, e.g., sudo apt-get install jq"
        return 1
      fi

      # Generate current command list
      eval "$artisan_cmd list --format=json" | jq -r '.commands[] | "\(.name)\t\(.description | gsub("\n"; " "))"' >"$current_command_list"

      # Cache invalidation check
      if [[ ! -f "$cache_file" || "$artisan_path" -nt "$cache_file" ||
        (-f "$composer_lock" && "$composer_lock" -nt "$cache_file") ]]; then
        mv -f "$current_command_list" "$cache_file"
      elif ! cmp -s "$current_command_list" "$cache_file"; then
        mv -f "$current_command_list" "$cache_file"
      else
        rm -f "$current_command_list"
      fi

      # Adjusted fzf command to display descriptions, exclude '_complete', and take only the first line of the description
      local selected_command=$(cat "$cache_file" | grep -v '_complete' | fzf --preview 'echo {} | awk "{\$1=\"\"; print substr(\$0,2)}"' --preview-window=right:50%:wrap --height=40% --reverse --prompt="Artisan Command > " --with-nth 1 --bind 'tab:accept' --query=$words[-1] | awk '{print $1}')
      ret=$?
      if [ -n "$selected_command" ]; then
        compadd -U -- "$selected_command"
      fi
      return $ret
    fi
    ;;
  args)
    if [[ -n "$artisan_path" ]]; then

      local output=$(eval "$words --help 2>&1")

      # Subcommands
      local artisan_subcommands=$(echo "$output" | awk '/^Available commands for the/{flag=1; next} flag && NF{print} !NF{flag=0}')

      # Arguments
      local artisan_arguments=$(echo "$output" | awk '/^Arguments:/{flag=1; next} flag && !/namespace/ && NF{print} !NF{flag=0}')

      # Flags
      local artisan_options=$(echo "$output" | awk '/^Options:/{flag=1; next} flag && NF{
        match($0, /--[a-zA-Z0-9-]+/);
        option = substr($0, RSTART, RLENGTH);
        value = substr($0, RSTART + RLENGTH + 1);
        trimmed_value = substr(value, index(value, " ") + 1)
        print option, trimmed_value
      } !NF{flag=0}')

      # Complete subcommands, arguments, and options if they exist
      if [[ -n "$artisan_subcommands" ]]; then
        local selected_subcommand=$(echo -e "$artisan_subcommands" | fzf --preview 'echo {} | awk "{\$1=\"\"; print substr(\$0,2)}"' --preview-window=right:50%:wrap --height=40% --reverse --prompt="Artisan Subcommand > " --with-nth 1 --bind 'tab:accept' --query=$words[-1] | awk '{print $1}')
        if [[ -n "$selected_subcommand" ]]; then
          # Extract the last word typed in the shell
          local last_word="${words[-1]}"

          # Check if the last word matches the beginning of the selected subcommand
          if [[ "$selected_subcommand" == "$last_word"* ]]; then
            # Remove the last word
            words=("${words[@]:0:$((${#words[@]} - 1))}")
          fi

          compadd -U -- $selected_subcommand
        fi
      # elif [[ -n "$artisan_arguments" ]]; then
      #   local selected_argument=$(echo -e "$artisan_arguments" | fzf --preview 'echo {} | awk "{\$1=\"\"; print substr(\$0,2)}"' --preview-window=right:50%:wrap --height=40% --reverse --prompt="Artisan Argument > " --with-nth 1 --bind 'tab:accept' | awk '{print $1}')
      #   if [[ -n "$selected_argument" ]]; then
      #     compadd -U -- $selected_argument
      #   fi
      elif [[ -n "$artisan_options" ]]; then
        local selected_option=$(echo -e "$artisan_options" | fzf --preview 'echo {} | awk "{\$1=\"\"; print substr(\$0,2)}"' --preview-window=right:50%:wrap --height=40% --reverse --prompt="Artisan Option > " --with-nth 1 --bind 'tab:accept' | awk '{print $1}')
        if [[ -n "$selected_option" ]]; then
          compadd -U -- $selected_option
        fi
      fi
    fi
    ;;
  *)
    _files
    ;;
  esac
}
