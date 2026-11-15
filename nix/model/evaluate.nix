# Public module evaluator. The caller owns the application module's location.
{
  model,
  lib ? import <nixpkgs/lib>,
}:
import ./lib/application.nix {
  inherit lib;
  modules = [model];
}
