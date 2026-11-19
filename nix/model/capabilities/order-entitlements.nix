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
  config = lib.mkMerge [
    (lib.mkIf config.orders.entitlements.enable {capabilities.order_entitlements.requires = ["orders" "entitlements"];})
    (lib.mkIf (config.orders.enable && config.entitlements.enable) {
      workflows.purchased_access = (import ../lib/reactions.nix {inherit config lib;}).purchasedAccess;
    })
  ];
}
