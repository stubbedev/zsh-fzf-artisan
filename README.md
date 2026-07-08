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
- Completes **argument and option values** by parsing your command's PHP source: values your code compares against `$this->argument()`/`$this->option()` (`===`, `in_array`, `match`, `switch`) become completion candidates — resolved through variable aliases, class constants, and backed enums
- Completes well-known Laravel values by argument/option name, all parsed statically from your project:
  - model classes (`--model=`), seeder classes (`db:seed --class=`), service providers (`vendor:publish --provider=`)
  - config keys, dotted (`config:show app.name`) and connection/store/disk/guard names
  - test names for `test --filter=` (PHPUnit classes, `test*`/`#[Test]`/`@test` methods, and Pest descriptions)
  - migration file paths (`migrate --path=`), HTTP methods (`route:list --method=`), environments (`--env=`)
- Optional: bridges to Laravel's own `_complete` for runtime-only values (route names, queue names, publish tags) when you opt in — see `ARTISAN_COMP_NATIVE` below
- With fzf: fuzzy picker with descriptions
- Without fzf: native zsh completion filtered by prefix
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

### Runtime value completion (opt-in)

Route names, queue names, publish tags, and option `suggestedValues` only exist once Laravel boots — no static parse can know them. Set `ARTISAN_COMP_NATIVE=1` to let the completer fall back to Laravel's built-in `_complete` for these:

```sh
# ~/.zshrc
export ARTISAN_COMP_NATIVE=1
```

It is consulted **only** when the static sources find nothing for the value you're completing, so it costs an artisan boot (~200-400ms) on those tabs and nothing on the rest. Off by default.

## How it works

`artisan.plugin.zsh` ensures the `artisan-comp` binary exists — downloading the release build matching this checkout's version (from `Cargo.toml`) into `bin/` in the background — and delegates completion requests to it. `git pull` upgrades both together; nothing to configure. Binaries are built in CI for every platform on tag push (`.github/workflows/release.yml`); the plugin never compiles anything on your machine.

The binary:

- Finds `artisan` by walking up the directory tree — no need to be in the project root
- Boots php exactly once per cache refresh: a single `artisan list --format=json` carries every command's full definition. Everything derived from your project is cached in `~/.cache/artisan` — the command list, per-command argument values extracted from your sources, and a project-wide catalog of well-known values (config keys, test names, migrations, models, …). A cached tab press takes ~1ms and never parses or boots anything
- Two independent invalidation signals so edits only rebuild what they affect: command-definition sources (Console dirs, `composer.lock`, `routes/console.php`, `bootstrap/app.php`) refresh the list/value caches; catalog sources (`config/`, `tests/`, `database/`, `app/Models`, `app/Providers`, `.env.*`) refresh the catalog. Editing a test never triggers an artisan re-list
- Completes position-aware: already-supplied positional arguments and already-typed options drop out of the suggestions
- Invalidates caches when `artisan`, `composer.lock`, or command sources change (`app/Console/Commands/`, `app/Modules/**/Console/`, `app/Console/Kernel.php`, `routes/console.php`, `bootstrap/app.php`)
- Discovers commands in `app/Console/Commands/`, `app/Modules/**/Console/`, `app/Console/Kernel.php`, and `routes/console.php`
- Parses your command sources with [mago](https://github.com/carthage-software/mago)'s PHP parser to extract valid values from comparisons (`===`/`!==`), `in_array()` (negated too), `match`, and `switch` — resolving variable aliases, same-file class constants, and backed enums (`Enum::Case->value`, `Enum::from()`/`tryFrom()`, `Enum::cases()`)
- Falls back to well-known sources by argument/option name and command: `model` → `app/Models`, `db:seed --class` → `database/seeders`, `vendor:publish --provider` → `app/Providers`, `connection`/`database`/`store`/`disk`/`guard` → config keys, `config:show` → dotted config keys, `test --filter` → test names, `migrate --path` → migration files, `route:list --method` → HTTP verbs, `--env` → `.env.*`
- Optionally bridges to Laravel's `_complete` for runtime-only values when `ARTISAN_COMP_NATIVE=1`, consulted only when static sources come up empty

## Releasing (maintainers)

Bump `version` in `Cargo.toml`, tag the commit `v<version>`, push the tag. CI builds Linux (musl) and macOS binaries for both architectures and attaches them to the GitHub release. Installed plugins detect the version change on their next shell load and download the new binary automatically.

## License

MIT — see [LICENSE](LICENSE).
