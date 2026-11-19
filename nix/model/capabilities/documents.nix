{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  document = config.capabilities.documents.entities.document or "business_document";
in {
  options.documents.enable = lib.mkEnableOption "immutable business document storage";
  config = lib.mkIf config.documents.enable {
    capabilities.documents.entities.document = lib.mkDefault "business_document";
    tables.${document} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      name = f.text;
      media_type = f.text;
      content_base64 = f.text // {metadata.privacy = "secret";};
      sha256 = f.text;
    };
    operations.${document} = import ../lib/private.nix;
  };
}
