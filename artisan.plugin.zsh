#--------------------------------------------------------------------------
# Laravel artisan plugin for zsh with fzf integration
#--------------------------------------------------------------------------
#
# Adds an `artisan` shell command that finds and executes Laravel's artisan
# from anywhere within the project, plus tab completions.
#
# Completions are served by the artisan-comp binary. This script's job is to
# ensure the binary exists — it downloads the prebuilt release matching this
# checkout's version from GitHub in the background — and to hand its output
# to zsh/fzf. No local toolchain is needed; `git pull` upgrades everything.
# fzf is optional — falls back to native zsh completion when not available.

# Cache setup
ARTISAN_CACHE_DIR="${HOME}/.cache/artisan"
mkdir -p "$ARTISAN_CACHE_DIR"

# Load zsh/datetime for $EPOCHSECONDS and strftime — used for cross-platform
# timestamp handling in the make: editor hook. Silent load.
zmodload -s zsh/datetime

# Cache fzf availability at load time — avoids a fork on every tab press.
if command -v fzf >/dev/null 2>&1; then
  _artisan_fzf_available() { return 0 }
else
  _artisan_fzf_available() { return 1 }
fi

# Resolve PHP binary at load time — completion subshells may have a stripped PATH
# (NixOS, Herd Lite, etc.) that doesn't include the same php as the interactive shell.
_ARTISAN_PHP_BIN="${_ARTISAN_PHP_BIN:-$(command -v php 2>/dev/null)}"
export _ARTISAN_PHP_BIN

# Per-$PWD artisan path cache — avoids directory walk on repeated calls.
typeset -gA _ARTISAN_FIND_CACHE

typeset -g _ARTISAN_PLUGIN_DIR="${${(%):-%N}:A:h}"
typeset -g _ARTISAN_COMP_BIN=""
typeset -g _ARTISAN_REPO="stubbedev/zsh-fzf-artisan"

#--------------------------------------------------------------------------
# artisan-comp binary management
#--------------------------------------------------------------------------

# Sets REPLY to the binary version this checkout wants (from Cargo.toml).
function _artisan_wanted_version() {
  REPLY=""
  local line
  while IFS= read -r line; do
    if [[ "$line" == version*=* ]]; then
      REPLY="${${line#*\"}%%\"*}"
      return 0
    fi
  done <"$_ARTISAN_PLUGIN_DIR/Cargo.toml" 2>/dev/null
  return 1
}

function _artisan_locate_binary() {
  local c
  # target/release is a local dev build escape hatch (unsupported platforms).
  for c in "$_ARTISAN_PLUGIN_DIR/bin/artisan-comp" "$_ARTISAN_PLUGIN_DIR/target/release/artisan-comp"; do
    if [[ -x "$c" ]]; then
      _ARTISAN_COMP_BIN="$c"
      return 0
    fi
  done
  return 1
}

# Download the release binary for this platform in the background. A stamp
# file records the attempted version so a failing download (offline, release
# missing) is retried only after `git pull` changes the wanted version, or
# after the stamp is deleted.
function _artisan_ensure_binary() {
  _artisan_wanted_version || return 1
  local wanted=$REPLY

  if _artisan_locate_binary; then
    # Dev builds in target/release are left alone; only bin/ is managed.
    [[ "$_ARTISAN_COMP_BIN" != "$_ARTISAN_PLUGIN_DIR/bin/"* ]] && return 0
    # Fork-free version check: the downloader stamps bin/.version. Fall back
    # to one exec (and backfill the stamp) if it's missing.
    local vfile="$_ARTISAN_PLUGIN_DIR/bin/.version" have=""
    if [[ -f "$vfile" ]]; then
      have="$(<$vfile)"
    else
      have=$("$_ARTISAN_COMP_BIN" version 2>/dev/null)
      [[ -n "$have" ]] && print -r -- "$have" >"$vfile" 2>/dev/null
    fi
    [[ "$have" == "$wanted" ]] && return 0
  fi

  local stamp="$ARTISAN_CACHE_DIR/download.stamp"
  [[ -f "$stamp" && "$(<$stamp)" == "$wanted" ]] && return 1

  local os arch
  case "$OSTYPE" in
    darwin*) os="apple-darwin" ;;
    linux*)  os="unknown-linux-musl" ;;
    *) return 1 ;;
  esac
  case "$(uname -m)" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64)  arch="x86_64" ;;
    *) return 1 ;;
  esac

  print -r -- "$wanted" >"$stamp"
  >&2 echo "zsh-fzf-artisan: downloading artisan-comp v${wanted} (${arch}-${os}) in background"
  (
    local base="https://github.com/${_ARTISAN_REPO}/releases/download/v${wanted}/artisan-comp-${arch}-${os}"
    local dest="$_ARTISAN_PLUGIN_DIR/bin/artisan-comp"
    local tmp="$_ARTISAN_PLUGIN_DIR/bin/.artisan-comp.$$.$RANDOM"
    local sum="$tmp.sha256"
    mkdir -p "$_ARTISAN_PLUGIN_DIR/bin"

    local fetch
    if command -v curl >/dev/null 2>&1; then
      fetch() { curl -fsSL -o "$1" "$2" }
    elif command -v wget >/dev/null 2>&1; then
      fetch() { wget -qO "$1" "$2" }
    else
      exit 1
    fi

    { fetch "$tmp" "$base" && fetch "$sum" "${base}.sha256" } || { rm -f "$tmp" "$sum"; exit 1 }

    # Verify the SHA-256 published alongside the binary before trusting it.
    # Refuse (rather than install unverified) if no checksum tool is available.
    local expected actual
    expected="${$(<"$sum")%% *}"
    if command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "$tmp")"; actual="${actual%% *}"
    elif command -v shasum >/dev/null 2>&1; then
      actual="$(shasum -a 256 "$tmp")"; actual="${actual%% *}"
    fi
    rm -f "$sum"
    if [[ -z "$expected" || -z "$actual" || "$actual" != "$expected" ]]; then
      rm -f "$tmp"
      exit 1
    fi

    chmod +x "$tmp"
    # Only install a binary that also runs and reports the expected version.
    if [[ "$("$tmp" version 2>/dev/null)" == "$wanted" ]]; then
      mv -f "$tmp" "$dest"
      print -r -- "$wanted" >"$_ARTISAN_PLUGIN_DIR/bin/.version"
      rm -f "$stamp"
    else
      rm -f "$tmp"
    fi
  ) &>/dev/null &!
  return 1
}

_artisan_ensure_binary

#--------------------------------------------------------------------------
# artisan command wrapper
#--------------------------------------------------------------------------

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

#--------------------------------------------------------------------------
# completions
#--------------------------------------------------------------------------

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
      if [[ "$selected" == '<'* ]]; then
        # Positional argument placeholder — insert "" with cursor between the quotes.
        LBUFFER+='""'
        (( CURSOR-- ))
      elif [[ "$selected" == *= ]]; then
        compadd -S '' -U -- "$selected"
      else
        compadd -U -- "$selected"
      fi
    fi
  else
    local -a eq_names eq_descs reg_entries pos_vals pos_disps
    while IFS=$'\t' read -r name desc; do
      [[ -z "$name" ]] && continue
      if [[ "$name" == '<'* ]]; then
        pos_vals+=('""')
        pos_disps+=("$name")
      elif [[ "$name" == *= ]]; then
        eq_names+=("$name")
        eq_descs+=("${desc:-$name}")
      else
        reg_entries+=("${name//:/\\:}:${desc:-$name}")
      fi
    done <<< "$items"
    # Options ending with = need -S '' to suppress the auto-inserted trailing space.
    [[ ${#eq_names} -gt 0 ]] && compadd -S '' -d eq_descs -- "${eq_names[@]}"
    [[ ${#reg_entries} -gt 0 ]] && _describe "$prompt" reg_entries
    # Positional args: insert "" so the user can type the value directly; -Q prevents quote-escaping.
    [[ ${#pos_vals} -gt 0 ]] && compadd -Q -S '' -d pos_disps -U -- "${pos_vals[@]}"
  fi
}

function _artisan() {
  # The binary may have finished downloading since plugin load — or still be
  # missing, in which case (re)trigger the background download and offer
  # nothing rather than block the prompt. Hint once per session so a dead tab
  # isn't a mystery.
  if [[ -z "$_ARTISAN_COMP_BIN" ]] && ! _artisan_locate_binary; then
    _artisan_ensure_binary
    if (( ! ${+_ARTISAN_DOWNLOAD_HINTED} )); then
      typeset -g _ARTISAN_DOWNLOAD_HINTED=1
      (( $+functions[_message] )) && _message "artisan completions: binary not ready yet (downloading in background)"
    fi
    return 1
  fi

  local out
  if ! out=$("$_ARTISAN_COMP_BIN" complete --cwd "$PWD" --current "$CURRENT" -- "${words[@]}" 2>/dev/null); then
    # Most common cause when the binary is present but fails: no php on PATH.
    if [[ -z "$_ARTISAN_PHP_BIN" ]] && (( ! ${+_ARTISAN_PHP_HINTED} )); then
      typeset -g _ARTISAN_PHP_HINTED=1
      (( $+functions[_message] )) && _message "artisan completions: php not found in PATH"
    fi
    return 1
  fi

  # First line is the prompt title, the rest are "candidate\tdescription" items.
  local prompt="${out%%$'\n'*}"
  local items="${out#*$'\n'}"
  [[ -z "$out" || -z "$items" || "$items" == "$out" ]] && return 0

  local last_word=$words[-1]
  if (( CURRENT == 2 )); then
    [[ $last_word = "artisan" || -z $last_word ]] && last_word=$words[-2]
    [[ $last_word = "artisan" ]] && last_word=""
  fi
  _artisan_complete "$prompt" "$last_word" "$items"
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

# `sail artisan ...` — same word-scan delegation as the php wrapper.
compdef _artisan_php_wrapper sail
