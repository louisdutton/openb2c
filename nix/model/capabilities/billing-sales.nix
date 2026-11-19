{
  config,
  lib,
  ...
}: {
  options.billing.payments.enable = lib.mkOption {
    type = lib.types.bool;
    default = config.billing.enable && config.payments.enable;
    description = "Account for each allocated bank transfer through customer deposits.";
  };
  options.billing.orders.enable = lib.mkOption {
    type = lib.types.bool;
    default = config.billing.enable && config.orders.enable && config.orders.payments.enable;
    description = "Invoice and account for paid orders.";
  };
  options.billing.subscriptions.enable = lib.mkOption {
    type = lib.types.bool;
    default = config.billing.enable && config.subscriptions.enable;
    description = "Invoice subscription payments with deferred revenue.";
  };
  config = lib.mkMerge [
    (lib.mkIf config.billing.payments.enable {
      capabilities.billing_payments.requires = ["billing" "payments"];
    })
    (lib.mkIf (config.billing.enable && config.payments.enable) {
      workflows.allocated_cash = (import ../lib/reactions.nix {inherit config lib;}).allocatedCash;
      workflows.reversed_cash = (import ../lib/reactions.nix {inherit config lib;}).reversedCash;
    })
    (lib.mkIf (config.billing.enable && config.orders.enable && config.orders.payments.enable) {
      workflows.order_billing = (import ../lib/reactions.nix {inherit config lib;}).orderBilling;
    })
    (lib.mkIf (config.billing.enable && config.subscriptions.enable) {
      workflows.subscription_billing = (import ../lib/reactions.nix {inherit config lib;}).subscriptionBilling;
    })
    (lib.mkIf config.billing.orders.enable {capabilities.billing_orders.requires = ["billing" "orders" "order_payments"];})
    (lib.mkIf config.billing.subscriptions.enable {capabilities.billing_subscriptions.requires = ["billing" "subscriptions"];})
  ];
}
