{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  customer = config.capabilities.customers.entities.customer or "customer_profile";
  account = config.capabilities.accounts.entities.account or "user";
in {
  options.customers.enable = lib.mkEnableOption "customer records and self-service signup";
  config = lib.mkIf config.customers.enable {
    capabilities.customers = {
      version = 2;
      requires = ["accounts"];
      entities.customer = lib.mkDefault "customer_profile";
    };
    tables.${customer} = {
      id = f.id;
      user_id = (f.fk account) // {unique = true;};
    };
    operations.${customer} = import ../lib/private.nix;
  };
}
