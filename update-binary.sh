#!/bin/sh
# Resolve the newest published artisan-comp release and install it into the
# cache. POSIX sh; used by the fish shim (backgrounded) and runnable by hand.
# Silent and best-effort — every failure exits 0 and is retried later via the
# same stamp throttles the zsh plugin uses (latest.stamp daily, download.stamp
# hourly). Shares the cache layout with the zsh plugin, so either shim keeps
# the binary fresh for both.
#
# Usage: update-binary.sh CACHE_DIR PLUGIN_DIR [REPO]
set -u
cache="${1:?cache dir}"
plugin="${2:?plugin dir}"
repo="${3:-stubbedev/zsh-fzf-artisan}"
bindir="$cache/bin"
command -v curl >/dev/null 2>&1 || exit 0
now=$(date +%s)

# Refresh the latest-release stamp at most once a day (releases/latest
# redirect: no API token, no rate limit). Keep the old version on failure.
stamp="$cache/latest.stamp"
old=""
ots=0
if [ -f "$stamp" ]; then
    read -r old ots <"$stamp" 2>/dev/null || true
fi
ots=${ots:-0}
if [ $((now - ots)) -ge 86400 ]; then
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "https://github.com/$repo/releases/latest" 2>/dev/null || true)
    ver="${url##*/tag/v}"
    [ "$ver" = "$url" ] && ver=""
    case "$ver" in '' | *[!0-9.]*) ver="$old" ;; esac
    printf '%s %s\n' "$ver" "$now" >"$stamp" 2>/dev/null || true
    old="$ver"
fi

# Wanted version: newest release, else the checkout's Cargo.toml floor.
wanted="$old"
if [ -z "$wanted" ] && [ -f "$plugin/Cargo.toml" ]; then
    wanted=$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$plugin/Cargo.toml" | head -1)
fi
[ -n "$wanted" ] || exit 0

have=""
[ -x "$bindir/artisan-comp" ] && have=$(cat "$bindir/.version" 2>/dev/null || true)
[ "$have" = "$wanted" ] && exit 0

# Throttle failed downloads to once an hour per wanted version.
dstamp="$cache/download.stamp"
if [ -f "$dstamp" ]; then
    sver=""
    sts=0
    read -r sver sts <"$dstamp" 2>/dev/null || true
    if [ "$sver" = "$wanted" ] && [ $((now - ${sts:-0})) -lt 3600 ]; then
        exit 0
    fi
fi
printf '%s %s\n' "$wanted" "$now" >"$dstamp" 2>/dev/null || true

case "$(uname -s)" in
Darwin) os="apple-darwin" ;;
Linux) os="unknown-linux-musl" ;;
*) exit 0 ;;
esac
case "$(uname -m)" in
arm64 | aarch64) arch="aarch64" ;;
x86_64 | amd64) arch="x86_64" ;;
*) exit 0 ;;
esac

mkdir -p "$bindir"
base="https://github.com/$repo/releases/download/v$wanted/artisan-comp-$arch-$os"
tmp="$bindir/.artisan-comp.$$"
if ! curl -fsSL -o "$tmp" "$base" || ! curl -fsSL -o "$tmp.sha256" "$base.sha256"; then
    rm -f "$tmp" "$tmp.sha256"
    exit 0
fi

# Verify the published SHA-256 before trusting the download; refuse (rather
# than install unverified) when no checksum tool exists.
expected=$(cut -d' ' -f1 <"$tmp.sha256")
rm -f "$tmp.sha256"
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp" | cut -d' ' -f1)
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp" | cut -d' ' -f1)
else
    actual=""
fi
if [ -z "$expected" ] || [ -z "$actual" ] || [ "$actual" != "$expected" ]; then
    rm -f "$tmp"
    exit 0
fi

chmod +x "$tmp"
# Only install a binary that runs and reports the expected version.
if [ "$("$tmp" version 2>/dev/null)" = "$wanted" ]; then
    mv -f "$tmp" "$bindir/artisan-comp"
    printf '%s\n' "$wanted" >"$bindir/.version"
    rm -f "$dstamp"
else
    rm -f "$tmp"
fi
