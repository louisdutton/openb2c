{
  config,
  lib,
  ...
}: let
  payment = config.capabilities.payments.entities.payment or "payment";
  event = config.capabilities.payments.entities.event or "payment_event";
  customer = config.capabilities.customers.entities.customer or "user";
in {
  options.payments.enable = lib.mkEnableOption "payments capability";
  config = lib.mkIf config.payments.enable {
    capabilities.payments = {
      requires = ["customers"];
      settings.provider = "fake";
      entities = {
        payment = lib.mkDefault "payment";
        event = lib.mkDefault "payment_event";
      };
    };
    tables.${payment} = {
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
      provider = {
        type = "text";
        required = true;
        constraints.enum = ["fake"];
      };
      reference = {
        type = "text";
        required = true;
        unique = true;
      };
      status = {
        type = "text";
        required = true;
        constraints.enum = ["pending" "succeeded" "failed" "cancelled" "refunded"];
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
    tables.${event} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      payment_id = {
        type = "integer";
        required = true;
        references = "${payment}(id)";
      };
      event_key = {
        type = "text";
        required = true;
        unique = true;
      };
      outcome = {
        type = "text";
        required = true;
        constraints.enum = ["succeeded" "failed" "refunded"];
      };
      result_json = {
        type = "text";
        required = true;
      };
      created_at = {
        type = "text";
        required = true;
      };
    };
    constraints.${payment}.unique.customer_request.fields = with config.refs.${payment}; [user_id request_key];
    operations.${payment} = import ../lib/managed.nix "payments.read";
    operations.${event} = import ../lib/private.nix;
  };
}
