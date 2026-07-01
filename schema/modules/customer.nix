{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
in
{
  tables.customer = {
    id = { type = "integer"; pk = true; auto = true; };
    name = { type = "text"; required = true; };
    email = { type = "text"; };
    phone = { type = "text"; };
    customer_type = { type = "text"; default = "'individual'"; };
    created_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
  };

  constraints.customer.unique_email = {
    kind = "unique";
    fields = [ config.refs.customer.email ];
    message = "Email must be unique.";
  };

  # CRUD operations are implicit (generated for all entities)
  # Only define custom operations here
  operations.customer = {
    upgrade_to_patron = {
      guard = E.and
        (E.ne (E.f "customer_type") (E.lit "patron"))
        (E.notNull (E.f "email"));
      set = {
        customer_type = "patron";
      };
      effects = [
        { notify = { channel = "email"; template = "patron_welcome"; }; }
      ];
    };
  };
}
