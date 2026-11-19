{
  config,
  lib,
  ...
}: let
  credential = config.capabilities.password_auth.entities.credential or "password_credential";
  session = config.capabilities.password_auth.entities.session or "password_session";
  attempt = config.capabilities.password_auth.entities.attempt or "password_attempt";
  customer = config.capabilities.accounts.entities.account or "user";
  id = {
    type = "integer";
    pk = true;
    auto = true;
  };
  owner = {
    type = "integer";
    required = true;
    references = "${customer}(id)";
  };
  secret = {
    type = "text";
    required = true;
    metadata.privacy = "secret";
  };
in {
  config = {
    capabilities.password_auth = {
      version = 2;
      requires = ["accounts"];
      entities = {
        credential = lib.mkDefault "password_credential";
        session = lib.mkDefault "password_session";
        attempt = lib.mkDefault "password_attempt";
      };
    };
    tables.${credential} = {
      inherit id;
      user_id = owner // {unique = true;};
      password_hash = secret;
    };
    tables.${session} = {
      inherit id;
      user_id = owner;
      token_hash = secret // {unique = true;};
      expires_at = {
        type = "text";
        required = true;
      };
      revoked = {
        type = "integer";
        required = true;
        default = "0";
        constraints.enum = ["0" "1"];
      };
    };
    tables.${attempt} = {
      inherit id;
      account_key = secret;
      peer_key = secret;
      created_at = {
        type = "text";
        required = true;
        default = "CURRENT_TIMESTAMP";
      };
    };
    operations.${credential} = import ../lib/private.nix;
    operations.${session} = import ../lib/private.nix;
    operations.${attempt} = import ../lib/private.nix;
  };
}
