# Extends user for B2C/customer contexts
{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.user;
  V = config.values.user;
  ER = config.effectRefs;
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

  effectContracts.notifications = {
    templates.patron_welcome = {};
    recipients.customer = {};
  };

  operations.user = {
    upgrade_to_patron = {
      guard = E.and
        (E.ne (E.f F.customer_type) (E.lit V.customer_type.patron))
        (E.notNull (E.f F.email));
      set.customer_type.value = V.customer_type.patron;
      effects = [
        { notify = { template = ER.notifications.templates.patron_welcome; to = ER.notifications.recipients.customer; }; }
      ];
    };
  };
}
