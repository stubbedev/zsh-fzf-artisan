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

  local laravel_path=$(dirname $artisan_path)
  local docker_compose_config_path=$(find $laravel_path -maxdepth 1 \( -name "docker-compose.yml" -o -name "docker-compose.yaml" \) | head -n1)
  local artisan_cmd

  if [ "$docker_compose_config_path" = '' ]; then
    artisan_cmd="php $artisan_path"
  else
    if [ "$(grep "laravel/sail" $docker_compose_config_path | head -n1)" != '' ]; then
      artisan_cmd="$laravel_path/vendor/bin/sail artisan"
    else
      local docker_compose_cmd=$(_docker_compose_cmd)
      local docker_compose_service_name=$($docker_compose_cmd ps --services 2>/dev/null | grep 'app\|php\|api\|workspace\|laravel\.test\|webhost' | head -n1)
      if [ -t 1 ]; then
        artisan_cmd="$docker_compose_cmd exec $docker_compose_service_name php artisan"
      else
        # The command is not being run in a TTY (e.g. it's being called by the completion handler below)
        artisan_cmd="$docker_compose_cmd exec -T $docker_compose_service_name php artisan"
      fi
    fi
  fi

  local artisan_start_time=$(date +%s)

  eval $artisan_cmd $*

  local artisan_exit_status=$? # Store the exit status so we can return it later

  if [[ $1 = "make:"* && $ARTISAN_OPEN_ON_MAKE_EDITOR != "" ]]; then
    # Find and open files created by artisan
    find \
      "$laravel_path/app" \
      "$laravel_path/tests" \
      "$laravel_path/database" \
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

      # Cache invalidation check
      if [[ ! -f "$cache_file" || "$artisan_path" -nt "$cache_file" ||
        (-f "$composer_lock" && "$composer_lock" -nt "$cache_file") ]]; then
        local docker_compose_config_path=$(find "$project_dir" -maxdepth 1 \( -name "docker-compose.yml" -o -name "docker-compose.yaml" \) | head -n1)
        local artisan_cmd

        if [ -z "$docker_compose_config_path" ]; then
          artisan_cmd="php $artisan_path"
        else
          if grep -q "laravel/sail" "$docker_compose_config_path"; then
            artisan_cmd="$project_dir/vendor/bin/sail artisan"
          else
            local docker_compose_cmd=$(_docker_compose_cmd)
            local service_name=$($docker_compose_cmd ps --services 2>/dev/null | grep 'app\|php\|api\|workspace\|laravel\.test\|webhost' | head -n1)
            artisan_cmd="$docker_compose_cmd exec -T $service_name php artisan"
          fi
        fi

        eval "$artisan_cmd list --raw" | awk '{print $1}' >"$cache_file"
      fi

      cat "$cache_file" | fzf --height=40% --reverse --prompt="Artisan Command > " | sed 's/\\:/:/g' | read -r line
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

function _docker_compose_cmd() {
  docker compose &>/dev/null
  if [ $? = 0 ]; then
    echo "docker compose"
  else
    echo "docker-compose"
  fi
}
