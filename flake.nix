{
  inputs = {
    nixpkgs.url = github:NixOS/nixpkgs/nixos-unstable-small;
    # Fancy '$ nix develop'
    devshell.url = github:numtide/devshell;

    # Few utils to iterate over supported systems.
    utils.url = github:numtide/flake-utils;

    shellac-server.url = github:gytis-ivaskevicius/shellac-server;
    #shellac-server.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, utils, devshell, shellac-server }:
    utils.lib.eachDefaultSystem (system:
      let
        shellac = shellac-server.defaultPackage.${system};

        pkgs = import nixpkgs {
          inherit system;
          overlays = [ devshell.overlay ];
        };

        mkIonExecScript = {shellacPath, shellacCompletionsPath, ionPath}: ''
          export PATH=$PATH:${shellacPath}:${ionPath}
          export SHELLAC_COMPLETIONS_DIR=${shellacCompletionsPath}
          ion
        '';
      in rec {

        defaultPackage = packages.ion;

        packages = rec {

          # Assumes ../shellac-server exists and it and ion is built using debug profile
          ion-shellac-local = pkgs.writeShellScriptBin "ion-shellac-local" (mkIonExecScript {
            ionPath = "$DEVSHELL_ROOT/target/debug";
            shellacPath = "$DEVSHELL_ROOT/../shellac-server/target/debug/";
            shellacCompletionsPath = "$DEVSHELL_ROOT/../shellac-server/completion";
          });

          # Builds ion/shellac
          ion-shellac = pkgs.writeShellScriptBin "ion-shellac" (mkIonExecScript {
            ionPath = ion + "/bin";
            shellacPath = shellac  + "/bin";
            shellacCompletionsPath = shellac + "/completion";
          });

          # Standalone ion shell
          ion = pkgs.rustPlatform.buildRustPackage rec {
            name = "ion-shell";
            src = ./.;
            nativeBuildInputs = with pkgs;[ capnproto ];
            # I was not able to figure out how to pull dependencies form git using naersk, thus I droped in the good old fashion cargoSha256
            cargoSha256 = "sha256-S9D0Z5EQWpI+JJ+rgMLTGk+m8W9C0q//fCHmxMvMI2E=";
          };

        };



        # nix develop
        devShell = pkgs.devshell.mkShell {
          name = "ion-shell";

          # Custom scripts. Also easy to use them in CI/CD
          commands = [
            {
              name = "fmt";
              help = "Check Nix formatting";
              command = "nixpkgs-fmt \${@} $DEVSHELL_ROOT";
            }
            {
              name = "run-ion";
              help = "Executes debug version of ion shell with shellac";
              command = packages.ion-shellac-local + "/bin/ion-shellac-local";
            }
          ];

          packages = with pkgs;[ nixpkgs-fmt capnproto rustc cargo stdenv.cc ];
        };
      });
}
