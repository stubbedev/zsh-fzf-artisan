# Laravel Artisan Zsh Plugin

This is a Zsh plugin that integrates Laravel's Artisan command-line tool with fzf for enhanced command execution and autocompletion. It allows you to run Artisan commands from anywhere within a Laravel project directory.

## Features

- Execute Laravel Artisan commands from any directory within a Laravel project.
- Autocompletion for Artisan commands using fzf.
- Automatically detects and uses Docker Compose or Laravel Sail if available.
- Opens files created by `artisan make:` commands in your preferred editor.

## Installation

1. **Clone the repository** into your custom plugins directory (e.g., `~/.oh-my-zsh/custom/plugins`):

   ```sh
   git clone <repository-url> ~/.oh-my-zsh/custom/plugins/artisan
   ```

2. **Add the plugin** to your `.zshrc` file:

   ```sh
   plugins=(... artisan)
   ```

3. **Reload your Zsh configuration**:

   ```sh
   source ~/.zshrc
   ```

## Usage

- Simply type `artisan` followed by any Artisan command you wish to execute.
- Use the fzf interface to search and autocomplete Artisan commands. The fzf interface provides a fuzzy search capability, allowing you to quickly find and select Artisan commands from a list.
- To trigger the fzf autocompletion, start typing an Artisan command and press `Tab`. This will open the fzf interface, where you can type to filter commands and press `Enter` to select one.

## Configuration

- Set the `ARTISAN_OPEN_ON_MAKE_EDITOR` environment variable to your preferred text editor to automatically open files created by `artisan make:` commands.

- You can customize the behavior of fzf by setting fzf-related environment variables. For example, you can change the height of the fzf window or the prompt text. Refer to the [fzf documentation](https://github.com/junegunn/fzf#environment-variables) for more details on available options.

  Example:

  ```sh
  export ARTISAN_OPEN_ON_MAKE_EDITOR="nvim" # For Visual Studio Code
  ```

## Requirements

- [fzf](https://github.com/junegunn/fzf) must be installed for command autocompletion.
- Docker and Docker Compose are required if you want to use the plugin with Dockerized Laravel projects.

## Notes

- The plugin caches Artisan command lists to improve performance. Cache is stored in `~/.cache/artisan`.
- Ensure that your Laravel project is set up correctly with Artisan and, if applicable, Docker Compose or Laravel Sail.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Please submit a pull request or open an issue to discuss any changes.

## Acknowledgments

- Inspired by the Laravel community and the need for efficient command-line tools.
