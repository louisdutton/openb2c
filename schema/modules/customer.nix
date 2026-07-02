{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.customer;
  V = config.values.customer;
in
{
  tables.customer = {
    id = { type = "integer"; pk = true; auto = true; };
    name = { type = "text"; required = true; };
    email = { type = "text"; };
    phone = { type = "text"; };
    customer_type = {
      type = "text";
      default = "'individual'";
      constraints.enum = [ "individual" "patron" "organization" ];
    };
    created_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
  };

  constraints.customer.unique.unique_email = {
    fields = with config.refs.customer; [ email ];
  };

  # CRUD operations are implicit (generated for all entities)
  # Only define custom operations here
  operations.customer = {
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
