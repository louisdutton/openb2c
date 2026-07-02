# Extends user for B2C/customer contexts
{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.user;
  V = config.values.user;
in
{
  tables.user = {
    customer_type = {
      type = "text";
      default = "'individual'";
      metadata = {
        label = "Customer type";
        displayPriority = 900;
      };
      constraints.enum = [ "individual" "patron" "organization" ];
    };
  };

  operations.user = {
    upgrade_to_patron = {
      guard = E.and
        (E.ne (E.f F.customer_type) (E.lit V.customer_type.patron))
        (E.notNull (E.f F.email));
      set = [
        { field = F.customer_type; value = V.customer_type.patron; }
      ];
      effects = [
        { notify = { channel = "email"; template = "patron_welcome"; }; }
      ];
    };
  };
}
