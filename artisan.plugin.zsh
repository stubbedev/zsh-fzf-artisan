#--------------------------------------------------------------------------
# Laravel artisan plugin for zsh with fzf integration
#--------------------------------------------------------------------------
#
# This plugin adds an `artisan` shell command that will find and execute
# Laravel's artisan command from anywhere within the project. It also
# adds shell completions that work anywhere artisan can be located.
# fzf is optional — falls back to native zsh completion when not available.

# Cache setup
ARTISAN_CACHE_DIR="${HOME}/.cache/artisan"
mkdir -p "$ARTISAN_CACHE_DIR"

# Load zsh/datetime for $EPOCHSECONDS and strftime — used to throttle the
# Commands file check and for cross-platform timestamp handling.
# Silent load: no error if the module is unavailable.
zmodload -s zsh/datetime

# Cache tool availability at load time — avoids a fork on every tab press.
if command -v fzf >/dev/null 2>&1; then
  _artisan_fzf_available() { return 0 }
else
  _artisan_fzf_available() { return 1 }
fi

if command -v jq >/dev/null 2>&1; then
  _artisan_jq_available() { return 0 }
else
  _artisan_jq_available() { return 1 }
fi

# Resolve PHP binary at load time — completion subshells may have a stripped PATH
# (NixOS, Herd Lite, etc.) that doesn't include the same php as the interactive shell.
_ARTISAN_PHP_BIN="${_ARTISAN_PHP_BIN:-$(command -v php 2>/dev/null)}"

# Global options present on every artisan command — filtered from args completions.
# Declared at load time so it is not reallocated on every tab press.
typeset -gr _ARTISAN_GLOBAL_OPTS='["help","quiet","verbose","version","ansi","no-ansi","no-interaction","env"]'

# Per-$PWD artisan path cache — avoids directory walk on repeated tab presses.
typeset -gA _ARTISAN_FIND_CACHE
# Per-string hash cache — pure-zsh hash only runs once per unique string per session.
typeset -gA _ARTISAN_HASH_CACHE
# Per-project command list cache — avoids disk reads on repeated command completion.
typeset -gA _ARTISAN_LIST_CACHE

function artisan() {
  _artisan_find || {
    >&2 echo "zsh-artisan: artisan not found. Are you in a Laravel directory?"
    return 1
  }
  local artisan_path=$REPLY

  # Use $EPOCHSECONDS (no fork) when zsh/datetime is loaded, else fall back to date.
  local artisan_start_time=${EPOCHSECONDS:-$(date +%s)}

  "${_ARTISAN_PHP_BIN:-php}" "$artisan_path" "$@"

  local artisan_exit_status=$?

  if [[ $1 = "make:"* && -n "$ARTISAN_OPEN_ON_MAKE_EDITOR" ]]; then
    local project_dir=${artisan_path:h}
    # BSD find (macOS) does not support GNU's -newermt flag. Cross-platform fix:
    # create a temp reference file stamped to artisan_start_time and use -newer.
    local ref time_str
    ref=$(mktemp) && {
      # strftime is a fork-free zsh builtin from zsh/datetime. Fall back to date
      # with the appropriate platform flag if the module was not loaded.
      if strftime -s time_str '%Y%m%d%H%M.%S' "$artisan_start_time" 2>/dev/null; then
        touch -t "$time_str" "$ref" 2>/dev/null
      elif [[ "$OSTYPE" == "darwin"* ]]; then
        touch -t "$(date -r "$artisan_start_time" +%Y%m%d%H%M.%S)" "$ref" 2>/dev/null
      else
        touch -d "@$artisan_start_time" "$ref" 2>/dev/null
      fi
      find "$project_dir/app" "$project_dir/tests" "$project_dir/database" \
        -type f -newer "$ref" \
        -exec "$ARTISAN_OPEN_ON_MAKE_EDITOR" {} \; 2>/dev/null
      rm -f "$ref"
    }
  fi

  return $artisan_exit_status
}

compdef _artisan artisan

# ./artisan — zsh strips path prefix for basename lookup, but register explicitly as fallback.
compdef _artisan './artisan'

# `php artisan ...` — find artisan in the word list and delegate.
function _artisan_php_wrapper() {
  local artisan_idx=0 i
  for (( i = 2; i <= ${#words}; i++ )); do
    if [[ "${words[$i]}" == "artisan" || "${words[$i]}" == *"/artisan" ]]; then
      artisan_idx=$i
      break
    fi
  done

  if (( artisan_idx > 0 )); then
    if (( CURRENT > artisan_idx )); then
      words=("artisan" "${words[@]:$artisan_idx}")
      (( CURRENT -= artisan_idx - 1 ))
      _artisan
    fi
    return
  fi

  _default
}
compdef _artisan_php_wrapper php

# Present tab-separated "name\tdescription" items as completions.
# With fzf: opens fuzzy picker. Without fzf: falls back to zsh _describe.
function _artisan_complete() {
  local prompt="$1" query="$2" items="$3"
  [[ -z "$items" ]] && return

  if _artisan_fzf_available; then
    local selected
    selected=$(fzf \
      --preview 'echo {2..}' \
      --preview-window=right:50%:wrap \
      --height=40% \
      --reverse \
      --prompt="$prompt > " \
      --delimiter=$'\t' \
      --with-nth=1 \
      --bind='tab:accept' \
      --query="$query" \
      <<< "$items")
    selected=${selected%%$'\t'*}
    if [[ -n "$selected" ]]; then
      # Suppress the auto-added space for options that take a value (end with =).
      if [[ "$selected" == *= ]]; then
        compadd -S '' -U -- "$selected"
      else
        compadd -U -- "$selected"
      fi
    fi
  else
    local -a eq_names eq_descs reg_entries
    while IFS=$'\t' read -r name desc; do
      [[ -z "$name" ]] && continue
      if [[ "$name" == *= ]]; then
        eq_names+=("$name")
        eq_descs+=("${desc:-$name}")
      else
        reg_entries+=("${name/:/\\:}:${desc:-$name}")
      fi
    done <<< "$items"
    # Options ending with = need -S '' to suppress the auto-inserted trailing space.
    [[ ${#eq_names} -gt 0 ]] && compadd -S '' -d eq_descs -- "${eq_names[@]}"
    [[ ${#reg_entries} -gt 0 ]] && _describe "$prompt" reg_entries
  fi
}

# Sets REPLY to the absolute artisan path. Returns 0 on success, 1 if not found.
# Cached per $PWD — directory walk only runs once per location per session.
# Re-validates the cached path on each hit to handle moves/deletes.
function _artisan_find() {
  if [[ "${+_ARTISAN_FIND_CACHE[$PWD]}" == "1" ]]; then
    REPLY="${_ARTISAN_FIND_CACHE[$PWD]}"
    if [[ -z "$REPLY" || -f "$REPLY" ]]; then
      [[ -n "$REPLY" ]] && return 0 || return 1
    fi
  fi
  local dir=.
  until [[ $dir -ef / ]]; do
    if [[ -f "$dir/artisan" ]]; then
      REPLY="${dir}/artisan"
      REPLY="${REPLY:A}"  # absolutize via :A — resolves . and .. without forking
      _ARTISAN_FIND_CACHE[$PWD]="$REPLY"
      return 0
    fi
    dir+=/..
  done
  _ARTISAN_FIND_CACHE[$PWD]=""
  REPLY=""
  return 1
}

# Sets REPLY to an 8-character hex hash of $1 for use as a cache filename component.
# Pure-zsh DJB2 variant — completely fork-free. Cached per unique string per session.
function _artisan_hash() {
  if [[ "${+_ARTISAN_HASH_CACHE[$1]}" == "1" ]]; then
    REPLY="${_ARTISAN_HASH_CACHE[$1]}"
    return
  fi
  local s="$1" c i
  local -i h=5381 code
  for (( i = 1; i <= ${#s}; i++ )); do
    c="${s[i]}"
    # printf '%d' "'$c" gives the ASCII value of $c via POSIX 'c format — printf is a zsh builtin, no fork.
    printf -v code '%d' "'$c"
    (( h = ((h << 5) + h + code) & 0x7fffffff ))
  done
  REPLY=$(printf '%08x' $h)
  _ARTISAN_HASH_CACHE[$1]="$REPLY"
}

# Return 0 (stale) if the cache file should be regenerated.
# Checks: missing/empty, artisan newer, composer.lock newer, Laravel command sources (throttled).
# $cmd_stamp is a project-level file that records the last time the Commands glob ran clean.
function _artisan_cache_stale() {
  local cache_file="$1" artisan_path="$2" project_dir="$3" composer_lock="$4" cmd_stamp="$5"
  [[ ! -f "$cache_file" || ! -s "$cache_file" ]] && return 0
  [[ "$artisan_path" -nt "$cache_file" ]] && return 0
  [[ -f "$composer_lock" && "$composer_lock" -nt "$cache_file" ]] && return 0

  # Commands glob: throttled to at most once per $ARTISAN_CMD_CHECK_INTERVAL seconds (default 10).
  # $EPOCHSECONDS requires zsh/datetime; falls back to always-check (safe) if unavailable.
  local now=${EPOCHSECONDS:-0}
  if (( now > 0 )); then
    local stamp_time=0
    [[ -f "$cmd_stamp" ]] && stamp_time=$(<"$cmd_stamp")
    (( now - stamp_time < ${ARTISAN_CMD_CHECK_INTERVAL:-10} )) && return 1
  fi

  # Limit the scan to Laravel command sources instead of recursing through the whole
  # project tree. This keeps macOS completions responsive on large repos.
  local -a cmd_files
  cmd_files=(
    $project_dir/app/**/Console/Commands/*.php(N-.)
    $project_dir/app/Console/Kernel.php(N-.)
    $project_dir/routes/console.php(N-.)
    $project_dir/bootstrap/app.php(N-.)
  )
  local f
  for f in $cmd_files; do
    [[ "$f" -nt "$cache_file" ]] && return 0
  done

  # Update stamp so the glob is skipped for the next $ARTISAN_CMD_CHECK_INTERVAL seconds.
  (( now > 0 )) && print -r -- "$now" >"$cmd_stamp"
  return 1
}

function _artisan() {
  local state

  _arguments '1: :->command' '*: :->args'

  _artisan_find || return
  local artisan_path=$REPLY

  if ! _artisan_jq_available; then
    >&2 echo "zsh-artisan: jq is not installed. Please install it to use completions."
    return 1
  fi

  local project_dir=${artisan_path:h}
  _artisan_hash "$project_dir"; local project_hash=$REPLY
  local cache_file="${ARTISAN_CACHE_DIR}/${project_hash}.cache"
  local composer_lock="${project_dir}/composer.lock"
  # Shared project-level stamp — throttles the Commands glob across all cache operations.
  local cmd_stamp="${ARTISAN_CACHE_DIR}/${project_hash}.stamp"

  case $state in
  command)
    # Use the partial word being typed, or fall back to previous word when cursor
    # is on the command token itself (e.g. `artisan <cursor>`).
    local last_word=$words[-1]
    [[ $last_word = "artisan" || -z $last_word ]] && last_word=$words[-2]
    [[ $last_word = "artisan" ]] && last_word=""

    if _artisan_cache_stale "$cache_file" "$artisan_path" "$project_dir" "$composer_lock" "$cmd_stamp"; then
      [[ -d "$ARTISAN_CACHE_DIR" ]] || mkdir -p "$ARTISAN_CACHE_DIR"
      # Filter internal commands (e.g. _complete) at write time — avoids grep on every tab.
      "${_ARTISAN_PHP_BIN:-php}" "$artisan_path" list --format=json 2>/dev/null \
        | jq -r '.commands[] | select(.name | startswith("_") | not) | "\(.name)\t\(.description | gsub("\n"; " "))"' \
        >"$cache_file"
      (( ${EPOCHSECONDS:-0} > 0 && -s $cache_file )) && print -r -- "$EPOCHSECONDS" >"$cmd_stamp"
      _ARTISAN_LIST_CACHE[$project_hash]=""
    fi

    if [[ -z "${_ARTISAN_LIST_CACHE[$project_hash]-}" && -s "$cache_file" ]]; then
      # $(<file) reads without forking. Cache in-memory for repeated completion calls.
      _ARTISAN_LIST_CACHE[$project_hash]="$(<"$cache_file")"
    fi

    [[ -n "${_ARTISAN_LIST_CACHE[$project_hash]-}" ]] && _artisan_complete "Artisan Command" "$last_word" "${_ARTISAN_LIST_CACHE[$project_hash]}"
    ;;
  args)
    # Use only the word currently being typed — no fallback to previous word.
    local last_word=$words[-1]

    local subcmd=$words[2]
    _artisan_hash "$subcmd"; local subcmd_hash=$REPLY
    local cmd_cache_file="${ARTISAN_CACHE_DIR}/${project_hash}_${subcmd_hash}.cmd"

    if _artisan_cache_stale "$cmd_cache_file" "$artisan_path" "$project_dir" "$composer_lock" "$cmd_stamp"; then
      [[ -d "$ARTISAN_CACHE_DIR" ]] || mkdir -p "$ARTISAN_CACHE_DIR"
      "${_ARTISAN_PHP_BIN:-php}" "$artisan_path" help "$subcmd" --format=json 2>/dev/null >"$cmd_cache_file"
      (( ${EPOCHSECONDS:-0} > 0 && -s $cmd_cache_file )) && print -r -- "$EPOCHSECONDS" >"$cmd_stamp"
    fi

    # Empty file means $subcmd is a namespace prefix or unknown — fall back to
    # prefix-filtered command list (fork-free zsh array operations).
    if [[ ! -s "$cmd_cache_file" ]]; then
      if [[ -f "$cache_file" ]]; then
        local -a all_cmds=("${(f)$(<"$cache_file")}")
        local ns_items=${(F)${(M)all_cmds:#${subcmd}:*}}
        [[ -n "$ns_items" ]] && _artisan_complete "Artisan Namespace" "$last_word" "$ns_items"
      fi
      return
    fi

    # Single jq pass: positional arguments + long options + shortcut aliases.
    local items
    items=$(jq -r --argjson skip_args '["command"]' --argjson skip_opts "$_ARTISAN_GLOBAL_OPTS" '
      def hints(v):
        (if (v.accept_value // false) and (v.is_value_required // true | not) then " [optional value]" else "" end) +
        (if v.is_multiple // false then " [repeatable]" else "" end) +
        (if (v.default != null and v.default != false and v.default != "" and (v.default | type) != "array")
         then " (default: " + (v.default | tostring) + ")" else "" end);
      (
        (.definition.arguments // {}) | to_entries[] |
        select(.value | type == "object") |
        select([.key] | inside($skip_args) | not) |
        "<" + .key +
          (if .value.is_array then "..." elif (.value.is_required | not) then "?" else "" end) +
        ">" + "\t" +
        (.value.description // "") + hints(.value)
      ),
      (
        (.definition.options // {}) | to_entries[] |
        select(.value | type == "object") |
        select([.key] | inside($skip_opts) | not) |
        (
          (.value.name + (if .value.accept_value then "=" else "" end)) + "\t" +
          (if (.value.shortcut // "") != "" then "(" + .value.shortcut + ") " else "" end) +
          (.value.description // "") + hints(.value)
        ),
        (
          select((.value.shortcut // "") != "") |
          (.value.shortcut + (if .value.accept_value then "=" else "" end)) + "\t" +
          (.value.description // "") + hints(.value)
        )
      )
    ' "$cmd_cache_file" 2>/dev/null)

    _artisan_complete "Artisan Args" "$last_word" "$items"
    ;;
  *)
    _files
    ;;
  esac
}
