{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.venue;
in
{
  tables.venue = {
    id = { type = "integer"; pk = true; auto = true; };
    name = { type = "text"; required = true; };
    address = { type = "text"; required = true; };
    city = { type = "text"; required = true; };
    postcode = { type = "text"; required = true; };
    capacity = { type = "integer"; required = true; };
    active = { type = "integer"; default = "1"; };  # SQLite boolean
    created_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
  };

  operations.venue = {
    read.public = true;

    deactivate = {
      guard = E.eq (E.f F.active) (E.lit 1);
      set.active.value = 0;
    };

    activate = {
      guard = E.eq (E.f F.active) (E.lit 0);
      set.active.value = 1;
    };
  };
}
