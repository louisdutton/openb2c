# Register all modules; only enabled modules contribute schema. Dependencies belong to consumers.
{
  lib,
  modules,
}: let
  model = lib.evalModules {
    modules = [../base.nix {_module.args.expr = import ./expr.nix;}] ++ builtins.attrValues (import ../capabilities) ++ modules;
  };
  inherit (model) config;
  compose = import ./compose.nix {inherit lib;};
  missing = lib.concatLists (lib.mapAttrsToList (
      name: capability:
        map (dependency: "${name} requires ${dependency}")
        (lib.filter (dependency: !(builtins.hasAttr dependency config.capabilities)) capability.requires)
    )
    config.capabilities);
in
  if missing != []
  then throw "Invalid capability composition: ${lib.concatStringsSep "; " missing}"
  else {
    inherit (config) organization currency capabilities workflows tables constraints derived seed auth audit integrations effectContracts presentation;
    operations = compose.processOperations config.tables config.relationships config.operations;
  }
