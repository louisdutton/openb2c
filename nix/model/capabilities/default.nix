{
  customers = ./customers.nix;
  password_auth = ./password-auth.nix;
  catalog = ./catalog.nix;
  orders = ./orders.nix;
  payments = ./payments.nix;
  subscriptions = ./subscriptions.nix;
  entitlements = ./entitlements.nix;
  order_payments = ./order-payments.nix;
  order_entitlements = ./order-entitlements.nix;
  subscription_entitlements = ./subscription-entitlements.nix;
}
