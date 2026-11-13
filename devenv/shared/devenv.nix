{
  pkgs,
  config,
  lib,
  inputs,
  ...
}: {
  options.openb2c.generator = lib.mkOption {
    type = lib.types.package;
    description = "Packaged OpenB2C generator shared by application environments.";
    default = import ../../nix/generator.nix {
      inherit pkgs lib;
      rustImport = config.languages.rust.import;
      toolchain = config.languages.rust.toolchainPackage;
      crate2nix = inputs.crate2nix;
    };
  };

  config = {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isLinux;
        message = "OpenB2C development environment supports Linux only.";
      }
    ];
    # The pinned nixpkgs channel provides Cargo, rustc, rustfmt, Clippy and rust-analyzer.
    languages.rust = {
      enable = true;
      channel = "nixpkgs";
    };
    languages.nix.enable = true;

    packages = [pkgs.sqlite pkgs.cloudflared pkgs.jq pkgs.chromium pkgs.chromedriver];
    env.CHROME_BINARY = "${pkgs.chromium}/bin/chromium";
    env.NIX_PATH = "nixpkgs=${pkgs.path}";

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

    outputs.generator = config.openb2c.generator;
    tasks."openb2c:test" = {
      description = "Run workspace, integration and Chromium browser tests";
      exec = ''
        cd "${config.git.root}"
        exec cargo test --workspace
      '';
    };
    tasks."devenv:enterTest".after = ["openb2c:test"];
  };
}
