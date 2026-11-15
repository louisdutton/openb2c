{
  config,
  lib,
  ...
}: {
  options.orders.entitlements.enable = lib.mkOption {
    type = lib.types.bool;
    default = config.orders.enable && config.entitlements.enable;
    description = "Grant and revoke purchased access when orders and entitlements are enabled.";
  };
  config = lib.mkIf config.orders.entitlements.enable {
    capabilities.order_entitlements.requires = ["orders" "entitlements"];
  };
}
