{
  config,
  lib,
  ...
}: let
  f = import ../lib/finance.nix;
  account = config.capabilities.accounting.entities.account or "ledger_account";
  journal = config.capabilities.accounting.entities.journal or "journal";
  line = config.capabilities.accounting.entities.line or "journal_line";
  period = config.capabilities.accounting.entities.period or "accounting_close";
in {
  options.accounting.enable = lib.mkEnableOption "double-entry accounting and financial reports";
  config = lib.mkIf config.accounting.enable {
    currency = lib.mkDefault "GBP";
    capabilities.accounting.entities = {
      account = lib.mkDefault "ledger_account";
      journal = lib.mkDefault "journal";
      line = lib.mkDefault "journal_line";
      period = lib.mkDefault "accounting_close";
    };
    tables.${account} = {
      id = f.id;
      code = f.text // {unique = true;};
      name = f.text;
      kind = f.enum ["asset" "liability" "equity" "income" "expense"];
    };
    tables.${journal} = {
      id = f.id;
      source_key = f.text // {unique = true;};
      request_json = f.text;
      date = f.text;
      currency = f.text;
      description = f.text;
      reverses_id =
        f.optionalInt
        // {
          references = "${journal}(id)";
          unique = true;
        };
    };
    tables.${line} = {
      id = f.id;
      journal_id = f.fk journal;
      account_id = f.fk account;
      debit = f.int;
      credit = f.int;
    };
    tables.${period} = {
      id = f.id;
      through_date = f.text // {unique = true;};
      created_at = f.text;
    };
    operations = lib.genAttrs [account journal line period] (_: import ../lib/private.nix);
    seed.reference.${account} =
      lib.imap0 (i: a: {
        id = i + 1;
        code = builtins.elemAt a 0;
        name = builtins.elemAt a 1;
        kind = builtins.elemAt a 2;
      }) [
        ["bank" "Bank" "asset"]
        ["clearing" "Payment clearing" "asset"]
        ["receivables" "Trade receivables" "asset"]
        ["tax_input" "Recoverable tax" "asset"]
        ["payables" "Trade payables" "liability"]
        ["tax_output" "Output tax" "liability"]
        ["deferred" "Deferred revenue" "liability"]
        ["sales" "Sales revenue" "income"]
        ["expenses" "Operating expenses" "expense"]
        ["equity" "Opening equity" "equity"]
      ]
      ++ [{code = "customer_deposits"; name = "Customer deposits"; kind = "liability";}];
  };
}
