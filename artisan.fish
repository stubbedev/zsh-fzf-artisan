# Laravel artisan plugin for fish.
#
# Adds an `artisan` command that finds and executes Laravel's artisan from
# anywhere within the project, with tab completion served by the artisan-comp
# binary (fish's own pager replaces fzf). The binary and its caches are shared
# with the zsh plugin; update-binary.sh keeps it fresh in the background.
#
# Install: source this file from config.fish, or symlink it into
# ~/.config/fish/conf.d/

status is-interactive; or exit

if not set -q ARTISAN_CACHE_DIR
    if set -q XDG_CACHE_HOME[1]; and test -n "$XDG_CACHE_HOME"
        set -gx ARTISAN_CACHE_DIR $XDG_CACHE_HOME/artisan
    else
        set -gx ARTISAN_CACHE_DIR $HOME/.cache/artisan
    end
end
mkdir -p $ARTISAN_CACHE_DIR

set -g __artisan_plugin_dir (dirname (realpath (status filename)))

# Resolve php at load — completion subshells may have a stripped PATH.
if not set -q _ARTISAN_PHP_BIN
    set -gx _ARTISAN_PHP_BIN (command -v php 2>/dev/null)
end

function __artisan_bin --description 'echo path of the artisan-comp binary'
    for c in $ARTISAN_CACHE_DIR/bin/artisan-comp \
        $__artisan_plugin_dir/bin/artisan-comp \
        $__artisan_plugin_dir/target/release/artisan-comp
        if test -x $c
            echo $c
            return 0
        end
    end
    return 1
end

# Keep the binary fresh: silent background updater, throttled by stamp files.
if test -f $__artisan_plugin_dir/update-binary.sh
    sh $__artisan_plugin_dir/update-binary.sh $ARTISAN_CACHE_DIR $__artisan_plugin_dir >/dev/null 2>&1 &
    disown 2>/dev/null
end

function __artisan_find --description 'echo path of the nearest artisan file'
    set -l dir $PWD
    while true
        if test -f $dir/artisan
            echo $dir/artisan
            return 0
        end
        test "$dir" = / ; and return 1
        set dir (dirname $dir)
    end
end

function artisan --description 'Laravel artisan from anywhere in the project'
    set -l artisan_path (__artisan_find)
    if test -z "$artisan_path"
        echo "artisan: artisan not found. Are you in a Laravel directory?" >&2
        return 1
    end
    set -l php $_ARTISAN_PHP_BIN
    test -n "$php"; or set php php
    $php $artisan_path $argv
end

function __artisan_complete_candidates
    set -l bin (__artisan_bin); or return
    set -l pre (commandline -opc)
    set -l cur (commandline -ct)
    # 1-based cursor word index, matching the zsh shim's $CURRENT.
    set -l current (math (count $pre) + 1)
    set -l out ($bin complete --cwd $PWD --current $current -- $pre $cur 2>/dev/null)
    test (count $out) -ge 2; or return
    # First line is the prompt title (fish shows no title; a MULTI prefix is
    # irrelevant here — fish completion is inherently re-invocable per token).
    set -l items $out[2..-1]
    if test "$items[1]" = __ARTISAN_FILES__
        __fish_complete_path $cur
        return
    end
    # Lines are already "candidate\tdescription" — exactly fish's format.
    printf '%s\n' $items
end

complete -c artisan -f -a '(__artisan_complete_candidates)'
