# Extends user for internal team contexts
{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.user;
  V = config.values.user;
in
{
  tables.user = {
    role = {
      type = "text";
      default = "'member'";
      constraints.enum = [ "admin" "member" "viewer" ];
    };
    status = {
      type = "text";
      default = "'active'";
      constraints.enum = [ "active" "inactive" "suspended" ];
    };
  };

  operations.user = {
    suspend = {
      relationships = [];
      guard = E.eq (E.f F.status) (E.lit V.status.active);
      set = [
        { field = F.status; value = V.status.suspended; }
      ];
      effects = [
        { notify = { channel = "email"; template = "account_suspended"; }; }
      ];
    };

    reactivate = {
      relationships = [];
      guard = E.or
        (E.eq (E.f F.status) (E.lit V.status.suspended))
        (E.eq (E.f F.status) (E.lit V.status.inactive));
      set = [
        { field = F.status; value = V.status.active; }
      ];
    };

    promote_to_admin = {
      relationships = [];
      guard = E.and
        (E.eq (E.f F.role) (E.lit V.role.member))
        (E.eq (E.f F.status) (E.lit V.status.active));
      set = [
        { field = F.role; value = V.role.admin; }
      ];
    };
  };
}
