#!/usr/bin/env zsh
# Headless test of the zsh shim. Stubs the compsys entry points (compadd,
# _describe, _files, compset, compdef, ...) as recording functions, serves a
# fixture project through a fake `php`, and drives _artisan / the php wrapper
# directly — no interactive shell, no zpty, no fzf (the fallback path is the
# one under test; the fzf path needs a terminal).
#
# Usage: zsh tests/zsh/completion_test.zsh /path/to/artisan-comp
emulate -L zsh
setopt no_unset pipe_fail

BIN=${1:?usage: completion_test.zsh /path/to/artisan-comp}
BIN=${BIN:A}
REPO=${0:A:h:h:h}
PLUGIN=$REPO/artisan.plugin.zsh
[[ -x $BIN ]] || { print -u2 "binary not executable: $BIN"; exit 1 }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# --- fixture project ---------------------------------------------------------
PROJ=$WORK/proj
mkdir -p $PROJ/app/Console/Commands
touch $PROJ/artisan
cat >$PROJ/app/Console/Commands/SyncCommand.php <<'EOF'
<?php
class SyncCommand extends Command
{
    protected $signature = 'app:sync {source} {--mode=} {--path=}';

    public function handle(): int
    {
        if (!in_array($this->argument('source'), ['github', 'gitlab'])) {
            return 1;
        }
        if (!in_array($this->option('mode'), ['fast', 'slow'])) {
            return 1;
        }
        return 0;
    }
}
EOF

# Fake php serving the `artisan list --format=json` the engine boots once.
mkdir -p $WORK/bin
cat >$WORK/list.json <<'EOF'
{"commands":[
 {"name":"app:sync","description":"Sync things","definition":{
   "arguments":{"source":{"is_array":false,"description":"The source"}},
   "options":{
     "mode":{"name":"--mode","shortcut":"","accept_value":true,"is_value_required":true,"is_multiple":false,"description":"Sync mode"},
     "path":{"name":"--path","shortcut":"","accept_value":true,"is_value_required":true,"is_multiple":false,"description":"A path"}
   }}},
 {"name":"migrate","description":"Run migrations","definition":{"arguments":{},"options":{}}}
]}
EOF
cat >$WORK/bin/php <<EOF
#!/bin/sh
cat "$WORK/list.json"
EOF
chmod +x $WORK/bin/php

# --- environment: no downloads, no real cache --------------------------------
export ARTISAN_CACHE_DIR=$WORK/cache
export _ARTISAN_PHP_BIN=$WORK/bin/php
mkdir -p $ARTISAN_CACHE_DIR/bin
ln -s $BIN $ARTISAN_CACHE_DIR/bin/artisan-comp
BINVER=$($BIN version)
print -r -- "$BINVER" >$ARTISAN_CACHE_DIR/bin/.version
zmodload zsh/datetime
print -r -- "$BINVER $EPOCHSECONDS" >$ARTISAN_CACHE_DIR/latest.stamp

# --- compsys stubs (before sourcing the plugin) -------------------------------
typeset -ga COMPADDED MESSAGES
typeset -g FILES_CALLED=0 DEFAULT_CALLED=0 PREV_PHP_CALLED=0

compadd() {
  while (( $# )); do
    case $1 in
      --) shift; COMPADDED+=("$@"); return 0 ;;
      -d) shift 2 ;;
      *) shift ;;
    esac
  done
  return 0
}
_describe() {
  # _describe title array-name — entries are "name:desc" with escaped colons.
  local -a entries
  entries=("${(@P)2}")
  local e n
  for e in "${entries[@]}"; do
    n=${e//\\:/$'\0'}
    n=${n%%:*}
    COMPADDED+=("${n//$'\0'/:}")
  done
}
_message() { MESSAGES+=("$*") }
_files()   { FILES_CALLED=1 }
_default() { DEFAULT_CALLED=1 }
compset()  { return 0 }
compdef()  { return 0 }
_prev_php() { PREV_PHP_CALLED=1 }
typeset -gA _comps
_comps[php]=_prev_php

# Source from a non-project dir so the load-time prewarm is a no-op.
cd $WORK
source $PLUGIN
# Force the fzf-less fallback path — the fzf picker needs a terminal.
_artisan_fzf_available() { return 1 }

cd $PROJ

# --- assertions ----------------------------------------------------------------
typeset -g fail=0
reset_recording() { COMPADDED=(); MESSAGES=(); FILES_CALLED=0; DEFAULT_CALLED=0; PREV_PHP_CALLED=0 }
assert_contains() {
  local name=$1 needle=$2
  if (( ${COMPADDED[(Ie)$needle]} )); then
    print "ok - $name"
  else
    print "FAIL - $name: wanted '$needle' in: ${COMPADDED[*]:-<empty>}"
    fail=1
  fi
}
assert_flag() {
  local name=$1 actual=$2
  if [[ $actual == 1 ]]; then
    print "ok - $name"
  else
    print "FAIL - $name"
    fail=1
  fi
}

typeset -ga words
typeset -g CURRENT service

# 1. command list
reset_recording
words=(artisan "") CURRENT=2
_artisan || true
assert_contains "command list offers app:sync" "app:sync"
assert_contains "command list offers migrate" "migrate"

# 2. args position: extracted positional values + declared options
reset_recording
words=(artisan app:sync "") CURRENT=3
_artisan || true
assert_contains "positional value extracted from source" "github"
assert_contains "option listed" "--mode="

# 3. inline option value
reset_recording
words=(artisan app:sync --mode=) CURRENT=3
_artisan || true
assert_contains "inline option value" "--mode=fast"

# 4. path-shaped option with no values → file completion
reset_recording
words=(artisan app:sync --path=) CURRENT=3
_artisan || true
assert_flag "FILES directive falls back to _files" $FILES_CALLED

# 5. php wrapper shifts words and completes artisan
reset_recording
words=(php artisan app:sync "") CURRENT=4 service=php
_artisan_php_wrapper || true
assert_contains "php wrapper delegates to artisan" "github"

# 6. non-artisan php delegates to the displaced completer
reset_recording
words=(php script.php) CURRENT=2 service=php
_artisan_php_wrapper || true
assert_flag "non-artisan php falls back to previous completer" $PREV_PHP_CALLED

if (( fail )); then
  print -u2 "zsh shim tests FAILED"
  exit 1
fi
print "zsh shim tests passed"
