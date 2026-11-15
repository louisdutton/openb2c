{
  pkgs,
  config,
  lib,
  inputs,
  ...
}: let
  generator = import ./nix/build/generator.nix {
    inherit pkgs lib;
    rustImport = config.languages.rust.import;
    toolchain = config.languages.rust.toolchainPackage;
    crate2nix = inputs.crate2nix;
  };
  publisher = import ./nix/build/generate.nix {
    inherit pkgs generator;
    toolchain = config.languages.rust.toolchainPackage;
    crate2nix = inputs.crate2nix;
    name = "publisher";
    schema = import ./nix/model/evaluate.nix {
      inherit lib;
      model = ./examples/publisher.nix;
    };
  };
in {
  assertions = [
    {
      assertion = pkgs.stdenv.hostPlatform.isLinux;
      message = "OpenB2C development environment supports Linux only.";
    }
  ];

  languages.rust = {
    enable = true;
    channel = "nixpkgs";
  };
  languages.nix.enable = true;
  packages = [pkgs.sqlite pkgs.cloudflared pkgs.jq];
  env = {
    NIX_PATH = "nixpkgs=${pkgs.path}";
    DB_PATH = lib.mkDefault "${config.git.root}/publisher.sqlite";
  };

  git-hooks.hooks = {
    rustfmt = {
      enable = true;
      always_run = true;
    };
    clippy = {
      enable = true;
      always_run = true;
      settings = {
        denyWarnings = true;
        offline = false;
        extraArgs = "--workspace --all-targets";
      };
    };
  };

  outputs = {
    inherit generator;
    publisher-generated = publisher;
    publisher = config.languages.rust.import publisher {};
  };
  processes = lib.optionalAttrs (!config.devenv.isTesting) {
    publisher = {
      exec = "${config.outputs.publisher}/bin/openb2c-app";
      cwd = config.git.root;
    };
  };
  tasks."openb2c:test" = {
    description = "Run workspace and backend integration tests";
    exec = ''
      cd "${config.git.root}"
      exec cargo test --workspace
    '';
  };
  tasks."devenv:enterTest".after = ["openb2c:test"];
}
