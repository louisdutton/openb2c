{
  config,
  lib,
  ...
}: let
  payment = config.capabilities.payments.entities.payment or "payment";
  event = config.capabilities.payments.entities.event or "payment_event";
  customer = config.capabilities.accounts.entities.account or "user";
  transaction = config.capabilities.payments.entities.transaction or "bank_transaction";
  allocation = config.capabilities.payments.entities.allocation or "payment_allocation";
  sync = config.capabilities.payments.entities.sync or "bank_feed_sync";
  f = import ../lib/finance.nix;
in {
  options.payments.enable = lib.mkEnableOption "payments capability";
  options.payments.feed = lib.mkOption {
    type = lib.types.enum ["simulated" "starling" "open_banking"];
    default = "simulated";
    description = "Read-only transaction source. The simulated source cannot contact a bank.";
  };
  options.payments.openBanking.referenceField = lib.mkOption {
    type = lib.types.enum ["none" "transaction_information" "transaction_reference"];
    default = "none";
    description = "Verified customer reference field for this bank's AIS implementation. The standard does not guarantee either field contains the payer's reference; none requires manual allocation.";
  };
  config = lib.mkIf config.payments.enable {
    capabilities.payments = {
      version = 2;
      requires = ["customers"];
      settings = {provider = "bank_transfer"; feed = config.payments.feed;}
        // lib.optionalAttrs (config.payments.feed == "open_banking") {
          reference_field = config.payments.openBanking.referenceField;
        };
      entities = {
        payment = lib.mkDefault "payment";
        event = lib.mkDefault "payment_event";
        transaction = lib.mkDefault "bank_transaction";
        allocation = lib.mkDefault "payment_allocation";
        sync = lib.mkDefault "bank_feed_sync";
      };
    };
    workflows.transfer_matching = (import ../lib/reactions.nix {inherit config lib;}).transferMatching;
    workflows.bank_reconciliation = {
      event = "tick";
      order = 100;
      steps = lib.optional (config.payments.feed != "simulated") {_t = "sync_transfers";}
        ++ [{_t = "reconcile_transfers";}];
    };
    tables.${payment} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      user_id = {
        type = "integer";
        required = true;
        references = "${customer}(id)";
      };
      amount_minor = {
        type = "integer";
        required = true;
        constraints.range.min = 1;
      };
      currency = {
        type = "text";
        required = true;
        constraints.pattern = "^[A-Z]{3}$";
      };
      provider = {
        type = "text";
        required = true;
        constraints.enum = ["bank_transfer" "fake"];
      };
      reference = {
        type = "text";
        required = true;
        unique = true;
      };
      status = {
        type = "text";
        required = true;
        constraints.enum = ["pending" "succeeded" "failed" "cancelled" "refunded"];
      };
      request_key = {
        type = "text";
        required = true;
      };
      created_at = {
        type = "text";
        required = true;
      };
      bank_reference = f.text // {default = "''";};
      bank_account = f.text // {default = "''";};
      beneficiary = f.text // {default = "''";};
      sort_code = f.text // {default = "''";};
      account_number = f.text // {default = "''";};
    };
    # Preserve historical event snapshots during upgrades. New settlement uses
    # immutable bank facts and allocations below, never synthetic outcomes.
    tables.${event} = {
      id = {
        type = "integer";
        pk = true;
        auto = true;
      };
      payment_id = {
        type = "integer";
        required = true;
        references = "${payment}(id)";
      };
      event_key = {
        type = "text";
        required = true;
        unique = true;
      };
      outcome = {
        type = "text";
        required = true;
        constraints.enum = ["succeeded" "failed" "refunded"];
      };
      result_json = {
        type = "text";
        required = true;
      };
      created_at = {
        type = "text";
        required = true;
      };
    };
    tables.${transaction} = {
      id = f.id;
      source = f.enum ["simulated" "starling" "open_banking"];
      account = f.text;
      transaction_id = f.text;
      amount_minor = f.int // {constraints.range.min = 1;};
      currency = f.text;
      direction = f.enum ["in" "out"];
      status = f.enum ["pending" "settled" "reversed"];
      reference = f.text;
      settled_at = f.optionalText;
      received_at = f.text;
      updated_at = f.text;
      review_reason = f.optionalText;
      bank_source = f.text // {default = "''";};
      source_reference = f.text // {default = "''";};
      narrative = f.text // {default = "''";};
      disposition = f.enum ["open" "excluded"] // {default = "'open'";};
      resolution_note = f.optionalText;
    };
    tables.${allocation} = {
      id = f.id;
      transaction_id = f.fk transaction;
      payment_id = f.fk payment;
      amount_minor = f.int // {constraints.range.min = 1;};
      kind = f.enum ["receipt" "refund"];
      created_at = f.text;
    };
    tables.${sync} = {
      id = f.id;
      changes_since = f.text;
      next_poll_at = f.text;
      requested = f.int;
      last_error = f.optionalText;
    };
    constraints.${transaction}.unique.bank_identity.fields = map (name: config.refs.${transaction}.${name}) ["source" "account" "transaction_id"];
    constraints.${allocation}.unique.transaction_payment.fields = map (name: config.refs.${allocation}.${name}) ["transaction_id" "payment_id"];
    operations.${transaction} = import ../lib/private.nix;
    operations.${allocation} = import ../lib/private.nix;
    operations.${sync} = import ../lib/private.nix;
    constraints.${payment}.unique.customer_request.fields = with config.refs.${payment}; [user_id request_key];
    operations.${payment} = import ../lib/managed.nix "payments.read";
    operations.${event} = import ../lib/private.nix;
  };
}
