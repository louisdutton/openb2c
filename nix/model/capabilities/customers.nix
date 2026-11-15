{
  config,
  lib,
  ...
}: {
  options.customers.enable = lib.mkEnableOption "customer records and credentials";
  config = lib.mkIf config.customers.enable (lib.mkMerge [
    (import ../modules/user.nix {inherit config lib;})
    (import ../modules/api_key.nix {inherit config lib;})
    {
      capabilities.customers.entities = {
        customer = "user";
        credential = "api_key";
      };
      operations.user = {
        create.enabled = false;
        update.enabled = false;
        delete.enabled = false;
      };
    }
  ]);
}
