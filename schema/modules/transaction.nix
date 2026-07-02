{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.transaction;
  V = config.values.transaction;
  ER = config.effectRefs;
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
      constraints.enum = [
        "pending"
        "authorized"
        "completed"
        "failed"
        "cancelled"
        "refund_required"
        "refunded"
      ];
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

  presentation.groups.paymentLifecycle = {
    label = "Payment lifecycle";
    description = "Payment settlement and refund operations.";
    displayPriority = 30;
  };

  effectContracts.notifications = {
    templates.receipt = {};
    templates.refund_confirmation = {};
    recipients.customer = {};
  };

  effectContracts.calls = {
    analytics.track_purchase = {};
    payment.process_refund = {};
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
        summary = "Completed transaction";
      };
      presentation = {
        label = "Complete transaction";
        description = "Payment settlement operation usually performed by a payment service or staff operator.";
        group = "paymentLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      transitions = [{
        field = F.status;
        from = with V.status; [ pending authorized ];
        to = V.status.completed;
      }];
      guard = E.or
        (E.eq (E.f F.status) (E.lit V.status.pending))
        (E.eq (E.f F.status) (E.lit V.status.authorized));
      set.status.value = V.status.completed;
      cascade = [{
        entity = config.entityRefs.ticket;
        via = config.entityRefs.transaction_ticket;
        set.status.value = ticketStatus.confirmed;
      }];
      effects = [
        { notify = { template = ER.notifications.templates.receipt; to = ER.notifications.recipients.customer; }; }
        { call = ER.calls.analytics.track_purchase; }
      ];
    };

    fail = {
      relationships = [];
      audit = {
        required = true;
        category = "payment";
        reason = "Payment failure affects order and ticket state.";
        summary = "Failed transaction";
      };
      presentation = {
        label = "Fail transaction";
        group = "paymentLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      transitions = [{
        field = F.status;
        from = with V.status; [ pending authorized ];
        to = V.status.failed;
      }];
      guard = E.or
        (E.eq (E.f F.status) (E.lit V.status.pending))
        (E.eq (E.f F.status) (E.lit V.status.authorized));
      set.status.value = V.status.failed;
      cascade = [{
        entity = config.entityRefs.ticket;
        via = config.entityRefs.transaction_ticket;
        set.status.value = ticketStatus.cancelled;
      }];
    };

    refund = {
      relationships = [];
      audit = {
        required = true;
        category = "payment";
        reason = "Refunds must be traceable for finance and support.";
        summary = "Refunded transaction";
      };
      presentation = {
        label = "Refund transaction";
        group = "paymentLifecycle";
        confirmation = {
          required = true;
          title = "Refund transaction";
          message = "This will start a payment refund and cancel linked tickets.";
          confirmLabel = "Refund";
          severity = "danger";
        };
      };
      policy = {
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      transitions = [{
        field = F.status;
        from = [ V.status.completed ];
        to = V.status.refunded;
      }];
      guard = E.eq (E.f F.status) (E.lit V.status.completed);
      set.status.value = V.status.refunded;
      cascade = [{
        entity = config.entityRefs.ticket;
        via = config.entityRefs.transaction_ticket;
        set.status.value = ticketStatus.cancelled;
      }];
      effects = [
        { notify = { template = ER.notifications.templates.refund_confirmation; to = ER.notifications.recipients.customer; }; }
        { call = ER.calls.payment.process_refund; }
      ];
    };
  };
}
