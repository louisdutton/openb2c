{
  config,
  lib,
  ...
}: let
  subscription = config.capabilities.subscriptions.entities.subscription or "subscription";
  attempt = config.capabilities.subscriptions.entities.attempt or "subscription_payment";
  customer = config.capabilities.customers.entities.customer or "user";
  price = config.capabilities.catalog.entities.price or "price";
  payment = config.capabilities.payments.entities.payment or "payment";
in {
  options.subscriptions.enable = lib.mkEnableOption "subscriptions capability";
  config = lib.mkIf config.subscriptions.enable {
    capabilities.subscriptions = {
      requires = ["customers" "catalog" "payments"];
      entities = {
        subscription = lib.mkDefault "subscription";
        attempt = lib.mkDefault "subscription_payment";
      };
    };
    tables.${subscription} = {
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
      price_id = {
        type = "integer";
        required = true;
        references = "${price}(id)";
      };
      product_code = {
        type = "text";
        required = true;
      };
      amount_minor = {
        type = "integer";
        required = true;
        constraints.range.min = 1;
      };
      currency = {
        type = "text";
        required = true;
        constraints.pattern = "^[A-Z]{3}$";
      };
      billing_interval = {
        type = "text";
        required = true;
        constraints.enum = ["month" "year"];
      };
      status = {
        type = "text";
        required = true;
        constraints.enum = ["pending" "active" "past_due" "cancelled"];
      };
      cancel_at_period_end = {
        type = "integer";
        required = true;
        default = "0";
        constraints.enum = ["0" "1"];
      };
      started_at = {type = "text";};
      current_period_start = {type = "text";};
      current_period_end = {type = "text";};
      cycle = {
        type = "integer";
        required = true;
        default = "-1";
      };
      request_key = {
        type = "text";
        required = true;
      };
      created_at = {
        type = "text";
        required = true;
      };
    };
    tables.${attempt} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      subscription_id = {
        type = "integer";
        required = true;
        references = "${subscription}(id)";
      };
      payment_id = {
        type = "integer";
        required = true;
        unique = true;
        references = "${payment}(id)";
      };
      cycle = {
        type = "integer";
        required = true;
        constraints.range.min = 0;
      };
      attempt = {
        type = "integer";
        required = true;
        constraints.range.min = 0;
      };
      request_key = {
        type = "text";
        required = true;
      };
    };
    constraints.${subscription}.unique.customer_request.fields = with config.refs.${subscription}; [user_id request_key];
    constraints.${attempt}.unique = {
      subscription_request.fields = with config.refs.${attempt}; [subscription_id request_key];
      cycle_attempt.fields = map (field: config.refs.${attempt}.${field}) ["subscription_id" "cycle" "attempt"];
    };
    operations.${subscription} = import ../lib/managed.nix "subscriptions.read";
    operations.${attempt} = import ../lib/private.nix;
  };
}
