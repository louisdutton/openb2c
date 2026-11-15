{
  config,
  lib,
  ...
}: let
  order = config.capabilities.orders.entities.order or "purchase_order";
  item = config.capabilities.orders.entities.item or "order_item";
  customer = config.capabilities.customers.entities.customer or "user";
  product = config.capabilities.catalog.entities.product or "product";
  price = config.capabilities.catalog.entities.price or "price";
in {
  options.orders.enable = lib.mkEnableOption "orders capability";
  config = lib.mkIf config.orders.enable {
    capabilities.orders = {
      requires = ["customers" "catalog"];
      entities = {
        order = lib.mkDefault "purchase_order";
        item = lib.mkDefault "order_item";
      };
    };
    tables.${order} = {
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
      status = {
        type = "text";
        required = true;
        constraints.enum = ["pending" "paid" "cancelled" "refunded"];
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
      request_key = {
        type = "text";
        required = true;
      };
      request_json = {
        type = "text";
        required = true;
      };
      created_at = {
        type = "text";
        required = true;
      };
    };
    tables.${item} = {
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
      product_id = {
        type = "integer";
        required = true;
        references = "${product}(id)";
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
      name = {
        type = "text";
        required = true;
      };
      quantity = {
        type = "integer";
        required = true;
        constraints.range = {
          min = 1;
          max = 100;
        };
      };
      unit_amount_minor = {
        type = "integer";
        required = true;
        constraints.range.min = 1;
      };
    };
    constraints.${order}.unique.customer_request.fields = with config.refs.${order}; [user_id request_key];
    operations.${order} = import ../lib/managed.nix "orders.read";
    operations.${item} = import ../lib/private.nix;
  };
}
