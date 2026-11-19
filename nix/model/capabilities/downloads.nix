{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  file = config.capabilities.downloads.entities.file or "download_file";
in {
  options.downloads.enable = lib.mkEnableOption "protected digital downloads";
  config = lib.mkIf config.downloads.enable {
    capabilities.downloads = {
      requires = ["entitlements"];
      entities.file = lib.mkDefault "download_file";
    };
    tables.${file} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      name = f.text;
      media_type = f.text;
      content_base64 = f.text // {metadata.privacy = "secret";};
      sha256 = f.text;
      size_bytes = f.int;
      resources_json = f.text;
      status = f.text // {constraints.enum = ["published" "withdrawn"];};
    };
    operations.${file} = import ../lib/private.nix;
  };
}
