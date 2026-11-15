{
  config,
  lib,
  ...
}: let
  product = config.capabilities.catalog.entities.product or "product";
  price = config.capabilities.catalog.entities.price or "price";
in {
  options.catalog.enable = lib.mkEnableOption "catalog capability";
  config = lib.mkIf config.catalog.enable {
    capabilities.catalog.entities = {
      product = lib.mkDefault "product";
      price = lib.mkDefault "price";
    };
    tables.${product} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      code = {
        type = "text";
        required = true;
        unique = true;
        constraints = {
          length = {
            min = 1;
            max = 100;
          };
          pattern = "^[a-z0-9][a-z0-9_-]*$";
        };
      };
      name = {
        type = "text";
        required = true;
        constraints.length = {
          min = 1;
          max = 160;
        };
      };
      description = {type = "text";};
      active = {
        type = "integer";
        required = true;
        default = "1";
        constraints.enum = ["0" "1"];
      };
    };
    tables.${price} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      product_id = {
        type = "integer";
        required = true;
        references = "${product}(id)";
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
        default = lib.mkIf (config.currency != null) "'${config.currency}'";
      };
      billing_interval = {
        type = "text";
        required = true;
        constraints.enum = ["one_off" "month" "year"];
      };
      active = {
        type = "integer";
        required = true;
        default = "1";
        constraints.enum = ["0" "1"];
      };
    };
    operations.${product}.read.public = true;
    operations.${price} = {
      read.public = true;
      update.enabled = false;
      delete.enabled = false;
    };
  };
}
