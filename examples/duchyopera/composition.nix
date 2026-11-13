# CLI entry point; Devenv outputs call model.nix with the pinned library directly.
import ./model.nix { lib = import <nixpkgs/lib>; }
