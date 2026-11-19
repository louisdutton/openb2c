{
  config,
  lib,
  ...
}: {
  options.tax = {
    enable = lib.mkEnableOption "UK VAT evidence and draft HMRC return reports";
    vat.registration = lib.mkOption {
      type = lib.types.enum ["unknown" "not_registered" "registered"];
      default = "unknown";
      description = "Explicit UK VAT registration status; unknown blocks a complete draft return.";
    };
  };
  config = lib.mkIf config.tax.enable {
    capabilities.tax = {
      requires = ["accounting"];
      settings = {
        jurisdiction = "GB";
        vatRegistration = config.tax.vat.registration;
      };
    };
  };
}
