{
  config,
  pkgs,
  lib,
  inputs,
  ...
}: let
  generated = import ../../nix/generate.nix {
    inherit pkgs;
    toolchain = config.languages.rust.toolchainPackage;
    crate2nix = inputs.crate2nix;
    generator = config.openb2c.generator;
    name = "duchyopera";
    schema = import ./model.nix {inherit lib;};
  };
in {
  outputs.duchyopera-generated = generated;
  outputs.duchyopera = config.languages.rust.import generated {};

  # Tests start their own isolated servers and databases.
  processes = lib.optionalAttrs (!config.devenv.isTesting) {
    duchyopera = {
      exec = "${config.outputs.duchyopera}/bin/openb2c-app";
      cwd = "${config.git.root}/examples/duchyopera";
    };
  };
}
