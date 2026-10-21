{
  perSystem = { config, lib, pkgs, ... }:
    let
      crate = (lib.importTOML ../Cargo.toml).package;
      artisan-comp = config.packages.artisan-comp;
    in
    {
      packages = {
        artisan-comp = pkgs.rustPlatform.buildRustPackage {
          pname = crate.name;
          inherit (crate) version;

          src = lib.fileset.toSource {
            root = ../.;
            fileset = lib.fileset.difference (lib.fileset.gitTracked ../.) (
              lib.fileset.unions [
                ../flake.nix
                ../flake.lock
                ../nix
              ]
            );
          };

          # Vendors deps straight from the committed Cargo.lock, so there is no
          # separate hash to bump — `cargo update` + commit is the only step.
          cargoLock.lockFile = ../Cargo.lock;

          meta = {
            inherit (crate) description;
            homepage = "https://github.com/stubbedev/zsh-fzf-artisan";
            license = lib.licenses.mit;
            mainProgram = crate.name;
            platforms = lib.platforms.unix;
          };
        };
        default = artisan-comp;
      };

      apps.default = {
        type = "app";
        program = lib.getExe artisan-comp;
      };

      checks.build = artisan-comp;
    };
}
