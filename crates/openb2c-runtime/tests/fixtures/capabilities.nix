{selection ? "full"}: let
  lib = import <nixpkgs/lib>;
  full = ["customers" "catalog" "orders" "payments" "subscriptions" "entitlements"];
  selected =
    {
      customers = ["customers"];
      catalog = ["catalog"];
      orders = ["customers" "catalog" "orders"];
      payments = ["customers" "payments"];
      subscriptions = ["customers" "catalog" "payments" "subscriptions"];
      entitlements = ["customers" "entitlements"];
      inherit full;
      renamed = full;
      broken = ["orders"];
      passwords = ["customers"];
      password_shop = full;
      broken_passwords = [];
      without_entitlements = lib.remove "entitlements" full;
      without_access = full;
      without_order_payments = full;
      broken_integration = ["customers" "catalog" "orders"];
    }.${
      selection
    };
in
  import ../../../../nix/model/lib/application.nix {
    inherit lib;
    modules = [
      (lib.genAttrs selected (_: {enable = true;}))
      {auth.password.enable = lib.elem selection ["passwords" "password_shop" "broken_passwords"];}
      (lib.optionalAttrs (selection == "without_access") {
        orders.entitlements.enable = false;
        subscriptions.entitlements.enable = false;
      })
      (lib.optionalAttrs (selection == "without_order_payments") {
        orders.payments.enable = false;
      })
      (lib.optionalAttrs (selection == "broken_integration") {
        orders.payments.enable = true;
      })
      ({config, ...}: {
        organization.name = "Capability tests";
        seed.reference = lib.optionalAttrs (lib.elem "catalog" selected) {
          ${config.capabilities.catalog.entities.product} = [
            {
              id = 1;
              code = "handbook";
              name = "Handbook";
              active = 1;
            }
            {
              id = 2;
              code = "membership";
              name = "Membership";
              active = 1;
            }
          ];
          ${config.capabilities.catalog.entities.price} = [
            {
              id = 1;
              product_id = 1;
              amount_minor = 1500;
              currency = "GBP";
              billing_interval = "one_off";
              active = 1;
            }
            {
              id = 2;
              product_id = 2;
              amount_minor = 500;
              currency = "GBP";
              billing_interval = "month";
              active = 1;
            }
            {
              id = 3;
              product_id = 2;
              amount_minor = 5000;
              currency = "GBP";
              billing_interval = "year";
              active = 1;
            }
            {
              id = 4;
              product_id = 1;
              amount_minor = 2000;
              currency = "EUR";
              billing_interval = "one_off";
              active = 1;
            }
          ];
        };
        capabilities = lib.optionalAttrs (selection == "renamed") {
          catalog.entities = {
            product = "offering";
            price = "offer_price";
          };
          orders.entities = {
            order = "checkout";
            item = "checkout_item";
          };
          payments.entities = {
            payment = "charge";
            event = "charge_event";
          };
          subscriptions.entities = {
            subscription = "agreement";
            attempt = "agreement_charge";
          };
          entitlements.entities.entitlement = "access_grant";
          order_payments.entities.link = "checkout_charge";
        };
      })
    ];
  }
