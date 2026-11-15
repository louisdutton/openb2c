{
  config,
  lib,
  ...
}: let
  entitlement = config.capabilities.entitlements.entities.entitlement or "entitlement";
  customer = config.capabilities.customers.entities.customer or "user";
in {
  options.entitlements.enable = lib.mkEnableOption "entitlements capability";
  config = lib.mkIf config.entitlements.enable {
    capabilities.entitlements = {
      requires = ["customers"];
      entities.entitlement = lib.mkDefault "entitlement";
    };
    tables.${entitlement} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      user_id = {
        type = "integer";
        required = true;
        references = "${customer}(id)";
      };
      resource = {
        type = "text";
        required = true;
      };
      source_key = {
        type = "text";
        required = true;
        unique = true;
      };
      status = {
        type = "text";
        required = true;
        constraints.enum = ["active" "revoked"];
      };
      starts_at = {
        type = "text";
        required = true;
      };
      ends_at = {type = "text";};
    };
    operations.${entitlement} = import ../lib/managed.nix "entitlements.read";
  };
}
