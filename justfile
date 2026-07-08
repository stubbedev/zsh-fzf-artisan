# justfile for zsh-fzf-artisan (Rust completion engine + zsh shim)
# Run `just` to see all available commands.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Default — list recipes.
default:
    @just --list --unsorted

# ─────────────────────────── Build & Test ───────────────────────────

# Build the release binary (the shim auto-uses target/release/artisan-comp).
build:
    cargo build --release
    @echo "Built target/release/artisan-comp"

fmt:
    cargo fmt

# Enable the pre-commit format gate (git hooksPath = .githooks). Idempotent.
install-hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    git config core.hooksPath .githooks
    echo "git config core.hooksPath = .githooks"
    echo "pre-commit 'cargo fmt --check' gate is now active (bypass with --no-verify)."

# Auto-fix formatting, then run the clippy gate (-D warnings).
lint: fmt
    cargo clippy --all-targets -- -D warnings

# Read-only lint gate matching CI: fmt --check + clippy -D warnings.
lint-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! cargo fmt --check; then
        echo "code is not formatted; run 'just fmt'"
        exit 1
    fi
    cargo clippy --all-targets -- -D warnings

test:
    cargo test

check: lint test

clean:
    cargo clean

# ─────────────────────────── Release ───────────────────────────

# Show what the next tag would be for each bump kind.
release-preview:
    #!/usr/bin/env bash
    set -euo pipefail
    CURRENT_TAG=$(git tag -l 'v*.*.*' --sort=-v:refname | head -1)
    CURRENT_TAG=${CURRENT_TAG:-v0.0.0}
    CURRENT_VERSION=${CURRENT_TAG#v}
    MAJOR=$(echo "$CURRENT_VERSION" | cut -d. -f1)
    MINOR=$(echo "$CURRENT_VERSION" | cut -d. -f2)
    PATCH=$(echo "$CURRENT_VERSION" | cut -d. -f3)
    echo "Current tag: $CURRENT_TAG"
    echo "  release-major: v$((MAJOR + 1)).0.0"
    echo "  release-minor: v${MAJOR}.$((MINOR + 1)).0"
    echo "  release-patch: v${MAJOR}.${MINOR}.$((PATCH + 1))"

_release-checks:
    #!/usr/bin/env bash
    set -euo pipefail
    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    DEFAULT_BRANCH=$(git rev-parse --abbrev-ref origin/HEAD 2>/dev/null | sed 's|^origin/||' || true)
    if [ -z "${DEFAULT_BRANCH:-}" ]; then
        DEFAULT_BRANCH=$(git remote show origin 2>/dev/null | awk '/HEAD branch/ {print $NF}' || echo master)
    fi
    if [ "$BRANCH" != "$DEFAULT_BRANCH" ]; then
        echo "Error: not on default branch '$DEFAULT_BRANCH' (currently '$BRANCH')." >&2
        exit 1
    fi
    just check
    if [ -n "$(git status --porcelain)" ]; then
        echo "Formatting/lint produced changes — staging + committing."
        git add -A
        git commit -m "chore: format code for release"
    fi

_release bump:
    #!/usr/bin/env bash
    set -euo pipefail
    just _release-checks
    CURRENT_TAG=$(git tag -l 'v*.*.*' --sort=-v:refname | head -1)
    CURRENT_TAG=${CURRENT_TAG:-v0.0.0}
    CURRENT_VERSION=${CURRENT_TAG#v}
    MAJOR=$(echo "$CURRENT_VERSION" | cut -d. -f1)
    MINOR=$(echo "$CURRENT_VERSION" | cut -d. -f2)
    PATCH=$(echo "$CURRENT_VERSION" | cut -d. -f3)
    case "{{bump}}" in
        major) NEW="$((MAJOR + 1)).0.0" ;;
        minor) NEW="${MAJOR}.$((MINOR + 1)).0" ;;
        patch) NEW="${MAJOR}.${MINOR}.$((PATCH + 1))" ;;
        *) echo "unknown bump kind: {{bump}}"; exit 1 ;;
    esac
    # Cargo.toml version MUST match the tag: release.yml refuses to publish
    # when `v$version` != the pushed tag. Bump both the manifest and the lock
    # before tagging so the CI guard passes and the shim's version check lines up.
    sed -i -E 's/^version = "[^"]*"/version = "'"$NEW"'"/' Cargo.toml
    cargo update -p artisan-comp
    git add Cargo.toml Cargo.lock
    git commit -m "chore: bump to v${NEW}"
    git tag -a "v${NEW}" -m "v${NEW}"
    git push origin HEAD
    git push origin "v${NEW}"
    echo
    echo "Tagged v${NEW}."
    echo "Watch the release build: gh run watch || open https://github.com/stubbedev/zsh-fzf-artisan/actions"

release-patch: (_release "patch")
release-minor: (_release "minor")
release-major: (_release "major")
