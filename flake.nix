{
  inputs = {
    # Fancy '$ nix develop'
    devshell.url = "github:numtide/devshell";

    # Few utils to iterate over supported systems.
    utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, utils, devshell }:
    utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ devshell.overlay ];
        };
      in
      rec {

        defaultPackage = pkgs.rustPlatform.buildRustPackage rec {
          name = "ion-shell";
          src = ./.;
          nativeBuildInputs = with pkgs;[ capnproto ];
          # I was not able to figure out how to pull dependencies form git using naersk, thus I droped in the good old fashion cargoSha256
          cargoSha256 = "sha256-S9D0Z5EQWpI+JJ+rgMLTGk+m8W9C0q//fCHmxMvMI2E=";
        };

        apps = {

          # Assumes ../shellac-server exists and is built using debug profile
          ion-shellac-debug = pkgs.writeShellScriptBin "ion-shellac-dev" ''
            export PATH=$PATH:../shellac-server/target/debug/
            ${defaultPackage}/bin/ion
          '';

          # Assumes ../shellac-server exists and is built using release profile
          ion-shellac-release = pkgs.writeShellScriptBin "ion-shellac-dev" ''
            export PATH=$PATH:../shellac-server/target/release/
            ${defaultPackage}/bin/ion
          '';

        };



        # nix develop
        devShell = pkgs.devshell.mkShell {
          name = "shellac-server";

          # Custom scripts. Also easy to use them in CI/CD
          commands = [
            {
              name = "fmt";
              help = "Check Nix formatting";
              command = "nixpkgs-fmt \${@} $DEVSHELL_ROOT";
            }
          ];

          packages = with pkgs;[ nixpkgs-fmt rustc cargo stdenv.cc ];
        };
      });
}
