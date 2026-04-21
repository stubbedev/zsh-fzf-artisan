# zsh-fzf-artisan

Run Laravel Artisan commands from anywhere in your project, with tab completion. [fzf](https://github.com/junegunn/fzf) is optional — works without it too.

## What it does

- Type `artisan` instead of `php artisan`, from any subdirectory of your project
- Press `Tab` to complete commands and arguments
- With fzf: fuzzy picker with descriptions
- Without fzf: native zsh completion filtered by prefix
- Automatically opens files created by `artisan make:` in your editor (optional)

## Requirements

- **zsh**
- **jq** — used to parse artisan's JSON output for completions
- **fzf** _(optional)_ — enables the fuzzy picker; falls back to standard zsh completion without it

### Install jq

| OS | Command |
|----|---------|
| macOS | `brew install jq` |
| Ubuntu/Debian | `sudo apt install jq` |
| Arch | `sudo pacman -S jq` |

### Install fzf (optional)

| OS | Command |
|----|---------|
| macOS | `brew install fzf` |
| Ubuntu/Debian | `sudo apt install fzf` |
| Arch | `sudo pacman -S fzf` |

## Installation

### Oh My Zsh

```sh
git clone https://github.com/stubbe/zsh-fzf-artisan ~/.oh-my-zsh/custom/plugins/artisan
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
zinit light stubbe/zsh-fzf-artisan
```

### Antigen

```sh
antigen bundle stubbe/zsh-fzf-artisan
```

### Manual

```sh
git clone https://github.com/stubbe/zsh-fzf-artisan ~/path/to/plugins/zsh-fzf-artisan
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

### Completion cache refresh interval

The plugin caches completions and checks for new commands every 10 seconds by default. Adjust with:

```sh
export ARTISAN_CMD_CHECK_INTERVAL=30  # seconds
```

## How it works

- Finds `artisan` by walking up the directory tree — no need to be in the project root
- Caches the command list per project in `~/.cache/artisan` — completions are fast after the first tab press
- Cache is invalidated automatically when `artisan` or `composer.lock` changes, or when you add new commands to `app/Console/Commands/`

## License

MIT — see [LICENSE](LICENSE).
