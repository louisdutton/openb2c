{
  config,
  lib,
  ...
}: let
  link = config.capabilities.order_payments.entities.link or "order_payment";
  order = config.capabilities.orders.entities.order or "purchase_order";
  payment = config.capabilities.payments.entities.payment or "payment";
in {
  options.orders.payments.enable = lib.mkOption {
    type = lib.types.bool;
    default = config.orders.enable && config.payments.enable;
    description = "Create and settle order payments when orders and payments are enabled.";
  };
  config = lib.mkIf config.orders.payments.enable {
    workflows = let
      r = import ../lib/reactions.nix {inherit config lib;};
    in {
      order_payment = r.orderPayment;
      order_cancellation = r.orderCancellation;
    };
    capabilities.order_payments = {
      requires = ["orders" "payments"];
      entities.link = lib.mkDefault "order_payment";
    };
    tables.${link} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      order_id = {
        type = "integer";
        required = true;
        references = "${order}(id)";
      };
      payment_id = {
        type = "integer";
        required = true;
        unique = true;
        references = "${payment}(id)";
      };
      request_key = {
        type = "text";
        required = true;
      };
    };
    constraints.${link}.unique.order_request.fields = with config.refs.${link}; [order_id request_key];
    operations.${link} = import ../lib/private.nix;
  };
}
