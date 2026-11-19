{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  supplier = config.capabilities.purchasing.entities.supplier or "supplier";
  expense = config.capabilities.purchasing.entities.expense or "supplier_expense";
  journal = config.capabilities.accounting.entities.journal or "journal";
  document = config.capabilities.documents.entities.document or "business_document";
in {
  options.purchasing.enable = lib.mkEnableOption "suppliers, evidenced expenses and payment recording";
  config = lib.mkIf config.purchasing.enable {
    capabilities.purchasing = {
      requires = ["accounting" "documents"];
      entities = {
        supplier = lib.mkDefault "supplier";
        expense = lib.mkDefault "supplier_expense";
      };
    };
    tables.${supplier} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      name = f.text;
      email = f.text;
    };
    tables.${expense} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      request_json = f.text;
      supplier_id = f.fk supplier;
      supplier_name = f.text;
      document_id = f.fk document;
      supplier_reference = f.text;
      description = f.text;
      date = f.text;
      posting_date = f.optionalText;
      currency = f.text;
      net_minor = f.int;
      tax_minor = f.int;
      tax_code = f.text;
      recoverable = f.int;
      journal_id = f.fk journal;
      payment_journal_id = f.optionalInt // {references = "${journal}(id)";};
      void_journal_id = f.optionalInt // {references = "${journal}(id)";};
    };
    constraints.${expense}.unique.supplier_invoice.fields = with config.refs.${expense}; [supplier_id supplier_reference];
    operations = lib.genAttrs [supplier expense] (_: import ../lib/private.nix);
  };
}
