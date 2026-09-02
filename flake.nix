{
  description = "artisan-comp — completion engine for zsh-fzf-artisan";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        artisan-comp = pkgs.rustPlatform.buildRustPackage {
          pname = "artisan-comp";
          version = "0.2.12";
          src = ./.;
          # Vendors deps straight from the committed Cargo.lock, so there is no
          # separate hash to bump — `cargo update` + commit is the only step.
          cargoLock.lockFile = ./Cargo.lock;

          meta = with pkgs.lib; {
            description = "Completion engine for zsh-fzf-artisan";
            homepage = "https://github.com/stubbedev/zsh-fzf-artisan";
            license = licenses.mit;
            mainProgram = "artisan-comp";
            platforms = platforms.unix;
          };
        };
      in
      {
        packages = {
          default = artisan-comp;
          artisan-comp = artisan-comp;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = artisan-comp;
          name = "artisan-comp";
        };

        checks.build = artisan-comp;

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            just
          ];
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
