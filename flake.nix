{
  description = "Mago is a toolchain for PHP that aims to provide a set of tools to help developers write better code.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-24.11";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, fenix, crane }: flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs {
        inherit system;
      };

      inherit (pkgs) lib;

      toolchain = fenix.packages.${system}.latest.toolchain;

      craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
      src = craneLib.cleanCargoSource ./.;

      commonArgs = {
        inherit src;
        strictDeps = true;

        buildInputs = [ toolchain ] ++ lib.optionals pkgs.stdenv.isDarwin [
          pkgs.libiconv
        ];
      };

      # Build *just* the cargo dependencies, so we can reuse
      # all of that work (e.g. via cachix) when running in CI
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      # Build the actual crate itself, reusing the dependency
      # artifacts from above.
      warudo = craneLib.buildPackage (commonArgs // {
        inherit cargoArtifacts;
      });
    in
    {
      checks = {
        inherit warudo;
      };

      packages.default = warudo;

      apps.default = flake-utils.lib.mkApp {
        drv = warudo;
      };

      devShells.default = craneLib.devShell {
        checks = self.checks.${system};

        packages = [ ];
      };
    });
}
