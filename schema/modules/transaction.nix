{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.transaction;
  V = config.values.transaction;
  ticket = config.refs.ticket;
  ticketStatus = config.values.ticket.status;
in
{
  tables.transaction = {
    id = { type = "integer"; pk = true; auto = true; };
    user_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      relationship = {
        label = "Customer";
        description = "Customer who owns this transaction.";
      };
    };
    amount_pence = { type = "integer"; required = true; };
    type = { type = "text"; required = true; };  # purchase, refund, donation
    status = {
      type = "text";
      default = "'pending'";
      constraints.enum = [ "pending" "completed" "failed" "refunded" ];
    };
    reference = { type = "text"; };  # External payment reference
    client = { type = "text"; default = "'web'"; };  # web, mcp, api
    created_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
  };

  constraints.transaction.unique.unique_reference = {
    fields = with config.refs.transaction; [ reference ];
  };

  # Link tickets to transactions
  tables.transaction_ticket = {
    id = { type = "integer"; pk = true; auto = true; };
    transaction_id = {
      type = "integer";
      required = true;
      references = "transaction(id)";
      relationship = {
        label = "Transaction";
      };
    };
    ticket_id = {
      type = "integer";
      required = true;
      references = "ticket(id)";
      relationship = {
        label = "Ticket";
      };
    };
  };

  constraints.transaction_ticket.unique.unique_pair = {
    fields = with config.refs.transaction_ticket; [
      transaction_id
      ticket_id
    ];
  };

  workflows.groups.paymentLifecycle = {
    label = "Payment lifecycle";
    description = "Payment settlement and refund operations.";
    displayPriority = 30;
  };

  audit.entities.transaction = {
    operations = [ "create" "update" "delete" "complete" "fail" "refund" ];
    category = "payment";
    reason = "Transactions represent payment and refund state.";
  };

  operations.transaction = {
    complete = {
      relationships = [];
      audit = {
        required = true;
        category = "payment";
        reason = "Payment settlement must be auditable.";
      };
      policy = {
        label = "Complete transaction";
        description = "Payment settlement operation usually performed by a payment service or staff operator.";
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      workflow = {
        group = "paymentLifecycle";
        transitions = [{
          field = F.status;
          from = [ V.status.pending ];
          to = V.status.completed;
        }];
        audit.summary = "Completed transaction";
      };
      guard = E.eq (E.f F.status) (E.lit V.status.pending);
      set = [
        { field = F.status; value = V.status.completed; }
      ];
      cascade = [{
        entity = "ticket";
        via = "transaction_ticket";
        set = [
          { field = ticket.status; value = ticketStatus.confirmed; }
        ];
      }];
      effects = [
        { notify = { channel = "email"; template = "receipt"; }; }
        { call = { service = "analytics"; action = "track_purchase"; }; }
      ];
    };

    fail = {
      relationships = [];
      audit = {
        required = true;
        category = "payment";
        reason = "Payment failure affects order and ticket state.";
      };
      policy = {
        label = "Fail transaction";
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      workflow = {
        group = "paymentLifecycle";
        transitions = [{
          field = F.status;
          from = [ V.status.pending ];
          to = V.status.failed;
        }];
        audit.summary = "Failed transaction";
      };
      guard = E.eq (E.f F.status) (E.lit V.status.pending);
      set = [
        { field = F.status; value = V.status.failed; }
      ];
      cascade = [{
        entity = "ticket";
        via = "transaction_ticket";
        set = [
          { field = ticket.status; value = ticketStatus.cancelled; }
        ];
      }];
    };

    refund = {
      relationships = [];
      audit = {
        required = true;
        category = "payment";
        reason = "Refunds must be traceable for finance and support.";
      };
      policy = {
        label = "Refund transaction";
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      workflow = {
        group = "paymentLifecycle";
        transitions = [{
          field = F.status;
          from = [ V.status.completed ];
          to = V.status.refunded;
        }];
        audit.summary = "Refunded transaction";
        confirmation = {
          required = true;
          title = "Refund transaction";
          message = "This will start a payment refund and cancel linked tickets.";
          confirmLabel = "Refund";
          severity = "danger";
        };
      };
      guard = E.eq (E.f F.status) (E.lit V.status.completed);
      set = [
        { field = F.status; value = V.status.refunded; }
      ];
      cascade = [{
        entity = "ticket";
        via = "transaction_ticket";
        set = [
          { field = ticket.status; value = ticketStatus.cancelled; }
        ];
      }];
      effects = [
        { notify = { channel = "email"; template = "refund_confirmation"; }; }
        { call = { service = "payment"; action = "process_refund"; }; }
      ];
    };
  };
}
