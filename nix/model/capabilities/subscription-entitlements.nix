{
  config,
  lib,
  ...
}: {
  options.subscriptions.entitlements.enable = lib.mkOption {
    type = lib.types.bool;
    default = config.subscriptions.enable && config.entitlements.enable;
    description = "Maintain paid access when subscriptions and entitlements are enabled.";
  };
  config = lib.mkMerge [
    (lib.mkIf config.subscriptions.entitlements.enable {capabilities.subscription_entitlements.requires = ["subscriptions" "entitlements"];})
    (lib.mkIf (config.subscriptions.enable && config.entitlements.enable) {
      workflows.subscription_access = (import ../lib/reactions.nix {inherit config lib;}).subscriptionAccess;
    })
  ];
}
