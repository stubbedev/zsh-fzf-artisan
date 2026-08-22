# zsh-fzf-artisan

Run Laravel Artisan commands from anywhere in your project, with tab completion. [fzf](https://github.com/junegunn/fzf) is optional — works without it too.

## Demo

Tab through commands, then through the values your code actually accepts — pulled from your command sources and Laravel conventions, not just the option list:

```text
$ artisan config:show <Tab>
╭──────────────────────────────────────────────────────────────────╮
│ Artisan Args >                    ┌────────────────────────────┐  │
│ > app.name                        │ (config value to display)  │  │
│   app.timezone                    │                            │  │
│   database.default                └────────────────────────────┘  │
│   database.connections.mysql                                      │
│   cache.stores.redis                                              │
│   5/128 ─────────────────────────────────────────────────────────│
╰──────────────────────────────────────────────────────────────────╯

$ artisan test --filter=<Tab>        # test classes, test* / #[Test] / @test methods, Pest descriptions
$ artisan app:sync <Tab>             # 'github', 'gitlab' — the values handle() compares against
$ artisan migrate --path=<Tab>       # migration files    $ artisan route:list --method=<Tab>  # GET, POST, …
```

Without fzf the same candidates come through native zsh completion, filtered by prefix.

## What it does

- Type `artisan` instead of `php artisan`, from any subdirectory of your project
- Press `Tab` to complete commands, arguments, and options
- Completes **argument and option values** by parsing your command's PHP source. Candidates come from:
  - values your code compares against `$this->argument()`/`$this->option()` (`===`, `in_array`, `match`, `switch`) — resolved through variable aliases, class constants, and backed enums
  - validation rules: `'mode' => 'required|in:fast,slow'` and `Rule::in([...])`
  - prompt fallbacks: `$this->option('env') ?? $this->choice('Which?', [...])` and Laravel Prompts `select()`/`multiselect()`
  - Symfony `complete()` overrides: `mustSuggestOptionValuesFor()` + `suggestValues([...])`
- Completes well-known Laravel values by argument/option name, all parsed statically from your project:
  - model classes (`--model=`), seeder classes (`db:seed --class=`), service providers (`vendor:publish --provider=`), event classes (`make:listener --event=`)
  - config keys, dotted (`config:show app.name`) and connection/store/disk/guard names
  - test names for `test --filter=` (PHPUnit classes, `test*`/`#[Test]`/`@test` methods, and Pest descriptions), suites for `test --testsuite=` (phpunit.xml), groups for `test --group=` (`#[Group]`/`@group`)
  - migration file paths (`migrate --path=`), table names from `Schema::create()` (`db:table`, `make:migration --table=`)
  - route names (`route:list --name=`), queue names from config/queue.php (`--queue=`)
  - HTTP methods (`route:list --method=`), environments (`--env=`)
- Path-shaped options with no known values (`--path=`, `--file=`) fall back to zsh file completion instead of a dead tab
- Repeatable options (`migrate --path=`) allow fzf multi-select — pick several with `Tab`, accept with `Enter`
- Optional: bridges to Laravel's own `_complete` for runtime-only values (publish tags, option `suggestedValues`) when you opt in — see `ARTISAN_COMP_NATIVE` below
- With fzf: fuzzy picker with descriptions
- Without fzf: native zsh completion filtered by prefix
- Also completes `php artisan ...`, `sail artisan ...`, and `herd php artisan ...` — non-artisan uses keep their original completions
- Fish shim included (`artisan.fish`) — same engine, fish's own pager
- Automatically opens files created by `artisan make:` in your editor (optional)

## Requirements

- **zsh**, **curl** (or wget)
- **fzf** _(optional)_ — enables the fuzzy picker; falls back to standard zsh completion without it

Completions are powered by a prebuilt binary (`artisan-comp`) that the plugin downloads automatically in the background on first load — no toolchain needed. Prebuilt targets: Linux (x86_64, aarch64, static musl) and macOS (x86_64, aarch64). On other platforms, build it yourself with `cargo build --release`; the plugin picks up `target/release/artisan-comp` automatically.

### Install fzf (optional)

| OS | Command |
|----|---------|
| macOS | `brew install fzf` |
| Ubuntu/Debian | `sudo apt install fzf` |
| Arch | `sudo pacman -S fzf` |

## Installation

### Oh My Zsh

```sh
git clone https://github.com/stubbedev/zsh-fzf-artisan ~/.oh-my-zsh/custom/plugins/artisan
```

Then add `artisan` to the plugins list in your `~/.zshrc`:

```sh
plugins=(git artisan)
```

Reload your shell:

```sh
source ~/.zshrc
```

### Zinit

```sh
zinit light stubbedev/zsh-fzf-artisan
```

### Antigen

```sh
antigen bundle stubbedev/zsh-fzf-artisan
```

### Manual

```sh
git clone https://github.com/stubbedev/zsh-fzf-artisan ~/path/to/plugins/zsh-fzf-artisan
```

Add to your `~/.zshrc`:

```sh
source ~/path/to/plugins/zsh-fzf-artisan/artisan.plugin.zsh
```

### Fish

```sh
git clone https://github.com/stubbedev/zsh-fzf-artisan ~/path/to/zsh-fzf-artisan
ln -s ~/path/to/zsh-fzf-artisan/artisan.fish ~/.config/fish/conf.d/artisan.fish
```

Same completion engine and cache; candidates render in fish's native pager (no fzf involved). The binary is kept fresh by `update-binary.sh` in the background.

## Usage

```sh
# Run any artisan command from anywhere inside your Laravel project
artisan migrate
artisan make:controller UserController

# Press Tab to complete commands
artisan ma<Tab>        # shows make:* commands
artisan migrate:<Tab>  # shows migrate:* subcommands

# Press Tab to complete arguments and options
artisan list --<Tab>   # shows available options

# Press Tab to complete values
artisan app:sync <Tab>            # values handle() actually checks for
artisan app:sync --mode=<Tab>     # from a switch/match/in_array/enum
artisan make:controller --model=<Tab>  # classes in app/Models
artisan cache:clear <Tab>         # store names from config/cache.php
artisan db:seed --class=<Tab>     # classes in database/seeders
artisan config:show <Tab>         # dotted config keys (app.name, ...)
artisan test --filter=<Tab>       # test classes/methods/descriptions
artisan migrate --path=<Tab>      # migration files
artisan route:list --method=<Tab> # GET, POST, ...
```

## Configuration

### Open generated files in your editor

Set `ARTISAN_OPEN_ON_MAKE_EDITOR` and any file created by `artisan make:*` will automatically open:

```sh
# ~/.zshrc
export ARTISAN_OPEN_ON_MAKE_EDITOR="code"    # VS Code
export ARTISAN_OPEN_ON_MAKE_EDITOR="nvim"    # Neovim
export ARTISAN_OPEN_ON_MAKE_EDITOR="phpstorm" # PhpStorm
```

### Cache location

Caches live in `~/.cache/artisan` by default, honoring `XDG_CACHE_HOME`. Override with `ARTISAN_CACHE_DIR`:

```sh
# ~/.zshrc, before the plugin loads
export ARTISAN_CACHE_DIR=~/some/where/artisan
```

### Troubleshooting

`artisan-comp doctor` prints everything the completer knows — binary version, php resolution, cache freshness, per-catalog counts:

```sh
~/.cache/artisan/bin/artisan-comp doctor --cwd /path/to/your/project
```

### Custom fzf flags

Set `ARTISAN_FZF_OPTS` to pass extra flags to the fzf picker (they override the defaults):

```sh
# ~/.zshrc
export ARTISAN_FZF_OPTS="--height=60% --preview-window=down:3:wrap"
```

### Completing an alias

If you alias artisan (e.g. `alias a=artisan`), register the completer for the alias too:

```sh
# ~/.zshrc (after the plugin loads)
compdef _artisan a
```

### Runtime value completion (opt-in)

Publish tags, option `suggestedValues`, and other values that only exist once Laravel boots can't always be parsed statically. Set `ARTISAN_COMP_NATIVE=1` to let the completer fall back to Laravel's built-in `_complete` for these:

```sh
# ~/.zshrc
export ARTISAN_COMP_NATIVE=1
```

It is consulted **only** when the static sources find nothing for the value you're completing, so it costs an artisan boot (~200-400ms) on those tabs and nothing on the rest — and results are cached for 60 seconds, so repeated tabs on the same value don't re-boot. Off by default.

## How it works

`artisan.plugin.zsh` ensures the `artisan-comp` binary exists — downloading the newest published release into `~/.cache/artisan/bin` in the background (see Releasing below for how the version is resolved) — and delegates completion requests to it. Nothing to configure. Binaries are built in CI for every platform on tag push (`.github/workflows/release.yml`); the plugin never compiles anything on your machine.

The binary:

- Finds `artisan` by walking up the directory tree — no need to be in the project root
- Boots php exactly once per cache refresh: a single `artisan list --format=json` carries every command's full definition. Everything derived from your project is cached in `~/.cache/artisan` — the command list, per-command argument values extracted from your sources, and a project-wide catalog of well-known values (config keys, test names, migrations, models, …). A cached tab press takes ~1ms and never parses or boots anything
- Two independent invalidation signals so edits only rebuild what they affect: command-definition sources (Console dirs, `composer.lock`, `routes/console.php`, `bootstrap/app.php`) refresh the list/value caches; catalog sources (`config/`, `tests/`, `database/`, `app/Models`, `app/Providers`, `app/Events`, `routes/`, `phpunit.xml`, `.env.*`) refresh the catalog. Editing a test never triggers an artisan re-list
- Completes position-aware: already-supplied positional arguments and already-typed options drop out of the suggestions
- Invalidates caches when `artisan`, `composer.lock`, or command sources change (`app/Console/Commands/`, `app/Modules/**/Console/`, `app/Console/Kernel.php`, `routes/console.php`, `bootstrap/app.php`)
- Discovers commands in `app/Console/Commands/`, `app/Modules/**/Console/`, `app/Console/Kernel.php`, and `routes/console.php`
- Parses your command sources with [mago](https://github.com/carthage-software/mago)'s PHP parser to extract valid values from comparisons (`===`/`!==`), `in_array()` (negated too), `match`, and `switch` — resolving variable aliases, same-file class constants, and backed enums (`Enum::Case->value`, `Enum::from()`/`tryFrom()`, `Enum::cases()`) — plus validation rules (`in:a,b`, `Rule::in`), prompt fallbacks (`?? $this->choice(...)`, `?: select(...)`), and Symfony `complete()` overrides (`suggestValues`)
- Falls back to well-known sources by argument/option name and command: `model` → `app/Models`, `db:seed --class` → `database/seeders`, `vendor:publish --provider` → `app/Providers`, `make:listener --event` → `app/Events`, `connection`/`database`/`store`/`disk`/`guard` → config keys, `config:show` → dotted config keys, `test --filter`/`--group`/`--testsuite` → test names/groups/suites, `migrate --path` → migration files, `table` → `Schema::create()` names, `route:list --name`/`--method` → route names/HTTP verbs, `--queue` → config/queue.php, `--env` → `.env.*`
- Synthesizes `test`'s phpunit passthrough options (`--filter`, `--group`, `--exclude-group`, `--testsuite`) that Laravel forwards without declaring
- Offers zsh file completion for path-shaped options with no known values, and fzf multi-select for repeatable options
- Optionally bridges to Laravel's `_complete` for runtime-only values when `ARTISAN_COMP_NATIVE=1`, consulted only when static sources come up empty (results cached for 60s)

## Releasing (maintainers)

Bump `version` in `Cargo.toml`, tag the commit `v<version>`, push the tag. CI builds Linux (musl) and macOS binaries for both architectures and attaches them to the GitHub release.

Installed plugins track the newest GitHub release automatically — no `git pull` required to receive a new binary. The shim resolves the latest release tag (via the `releases/latest` redirect, throttled to once a day) and silently downloads the matching binary in the background on a subsequent shell; the update is checksum-verified and invisible to the user. The `Cargo.toml` version is only a fallback used before the first successful lookup (fresh clone, offline, or `curl` unavailable). Changes to the plugin's own `.zsh` script still require updating the checkout.

Release binaries carry signed GitHub build provenance. To verify one:

```sh
gh attestation verify artisan-comp-x86_64-unknown-linux-musl --repo stubbedev/zsh-fzf-artisan
```

## License

MIT — see [LICENSE](LICENSE).
