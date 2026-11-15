# Helper: evaluates all available modules (useful for testing/development)
# Application modules should use evaluate.nix instead.
let
  lib = import <nixpkgs/lib>;
  composeLib = import ./lib/compose.nix { inherit lib; };

  # Auto-discover all modules
  moduleFiles =
    let dir = ./modules;
    in map (f: dir + "/${f}") (builtins.attrNames (builtins.readDir dir));

  modules = lib.evalModules {
    modules = [
      ./base.nix
    ] ++ moduleFiles;
  };

in {
  organization = modules.config.organization;
  integrations = modules.config.integrations;
  effectContracts = modules.config.effectContracts;
  presentation = modules.config.presentation;
  tables = modules.config.tables;
  refs = modules.config.refs;
  entityRefs = modules.config.entityRefs;
  relationships = modules.config.relationships;
  constraints = modules.config.constraints;
  operations = composeLib.processOperations modules.config.tables modules.config.relationships modules.config.operations;
  capabilities = modules.config.capabilities;
}
