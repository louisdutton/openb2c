# Platform identity is always present. Business capabilities do not own credentials.
{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  staff = config.capabilities.accounts.entities.staff or "staff_member";
in {
  config = lib.mkMerge [
    (import ../modules/user.nix {inherit config lib;})
    (import ../modules/api_key.nix {inherit config lib;})
    {
      capabilities.accounts.entities = {
        account = "user";
        credential = "api_key";
        staff = lib.mkDefault "staff_member";
      };
      tables.${staff} = {
        id = f.id;
        user_id = (f.fk "user") // {unique = true;};
        scopes = f.text;
        active =
          f.int
          // {
            default = "1";
            constraints.enum = ["0" "1"];
          };
      };
      operations.user = import ../lib/private.nix;
      operations.${staff} = import ../lib/private.nix;
    }
  ];
}
