{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  statement = config.capabilities.banking.entities.statement or "bank_statement_line";
  account = config.capabilities.accounting.entities.account or "ledger_account";
  line = config.capabilities.accounting.entities.line or "journal_line";
in {
  options.banking.enable = lib.mkEnableOption "bank statement import and exact ledger reconciliation";
  config = lib.mkIf config.banking.enable {
    capabilities.banking = {
      requires = ["accounting"];
      entities.statement = lib.mkDefault "bank_statement_line";
    };
    tables.${statement} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      account_id = f.fk account;
      date = f.text;
      currency = f.text;
      amount_minor = f.int;
      description = f.text;
      journal_line_id =
        f.optionalInt
        // {
          references = "${line}(id)";
          unique = true;
        };
    };
    operations.${statement} = import ../lib/private.nix;
  };
}
