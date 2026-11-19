{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  invoice = config.capabilities.billing.entities.invoice or "invoice";
  credit = config.capabilities.billing.entities.credit or "credit_note";
  receipt = config.capabilities.billing.entities.receipt or "invoice_receipt";
  refund = config.capabilities.billing.entities.refund or "invoice_refund";
  event = config.capabilities.billing.entities.event or "billing_event";
  journal = config.capabilities.accounting.entities.journal or "journal";
in {
  options.billing.enable = lib.mkEnableOption "invoices, credit notes and revenue recognition";
  options.billing.taxCode = lib.mkOption {
    type = lib.types.enum ["unassessed" "standard" "reduced" "zero" "exempt" "outside" "not_registered"];
    default = "unassessed";
    description = "Explicit UK VAT treatment for automatic sales invoices; amounts are VAT inclusive.";
  };
  config = lib.mkIf config.billing.enable {
    capabilities.billing = {
      version = 2;
      requires = ["accounting"];
      settings.defaultTaxCode = config.billing.taxCode;
      entities = {
        invoice = lib.mkDefault "invoice";
        credit = lib.mkDefault "credit_note";
        receipt = lib.mkDefault "invoice_receipt";
        refund = lib.mkDefault "invoice_refund";
        event = lib.mkDefault "billing_event";
      };
    };
    tables.${invoice} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      request_json = f.text;
      issuer = f.text;
      customer = f.text;
      description = f.text;
      date = f.text;
      due_date = f.text;
      posting_date = f.optionalText;
      currency = f.text;
      net_minor = f.int;
      tax_minor = f.int;
      tax_code = f.text;
      period_start = f.optionalText;
      period_end = f.optionalText;
      recognized_minor = f.int // {default = "0";};
      journal_id = f.fk journal;
    };
    tables.${credit} = {
      id = f.id;
      invoice_id = (f.fk invoice) // {unique = true;};
      date = f.text;
      reason = f.text;
      journal_id = f.fk journal;
    };
    tables.${receipt} = {
      id = f.id;
      invoice_id = (f.fk invoice) // {unique = true;};
      date = f.text;
      journal_id = f.fk journal;
    };
    tables.${refund} = {
      id = f.id;
      invoice_id = (f.fk invoice) // {unique = true;};
      date = f.text;
      reason = f.text;
      journal_id = f.fk journal;
    };
    tables.${event} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      payload_json = f.text;
      occurred_at = f.text;
      status = f.enum ["pending" "posted"];
    };
    operations = lib.genAttrs [invoice credit receipt refund event] (_: import ../lib/private.nix);
  };
}
