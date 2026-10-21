{
  perSystem = { pkgs, ... }: {
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
  };
}
