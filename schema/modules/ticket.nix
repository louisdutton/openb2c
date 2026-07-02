{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.ticket;
  V = config.values.ticket;
  performance = config.entityRefs.performance;
  performanceStatus = config.values.performance.status;
in
{
  tables.ticket = {
    id = { type = "integer"; pk = true; auto = true; };
    performance_id = {
      type = "integer";
      required = true;
      references = "performance(id)";
      metadata = {
        label = "Performance";
        displayPriority = 10;
      };
      relationship = {
        label = "Performance";
        description = "Performance this ticket admits the customer to.";
      };
    };
    user_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      metadata = {
        label = "Customer";
        displayPriority = 20;
      };
      relationship = {
        label = "Customer";
        description = "Customer who owns this ticket.";
      };
    };
    seat = {
      type = "text";
      metadata = {
        label = "Seat";
        placeholder = "A12";
        displayPriority = 30;
      };
      constraints.length.max = 24;
    };
    price_pence = {
      type = "integer";
      required = true;
      metadata = {
        label = "Price";
        format = "money";
        displayPriority = 40;
      };
      constraints.range.min = 0;
    };
    ticket_type = {
      type = "text";
      default = "'standard'";
      metadata = {
        label = "Ticket type";
        displayPriority = 50;
      };
      constraints.enum = [ "standard" "concession" "patron" "vip" ];
    };
    status = {
      type = "text";
      default = "'reserved'";
      metadata = {
        label = "Status";
        displayPriority = 60;
      };
      constraints.enum = [ "reserved" "confirmed" "cancelled" "used" ];
    };
    created_at = {
      type = "text";
      default = "CURRENT_TIMESTAMP";
      metadata = {
        label = "Created";
        format = "date-time";
        displayPriority = 1000;
      };
    };
  };

  constraints.ticket.unique.unique_performance_seat = {
    fields = with config.refs.ticket; [
      performance_id
      seat
    ];
  };

  constraints.ticket.check.vip_price_minimum = {
    fields = with config.refs.ticket; [
      ticket_type
      price_pence
    ];
    expression = E.or
      (E.ne (E.f F.ticket_type) (E.lit V.ticket_type.vip))
      (E.gte (E.f F.price_pence) (E.lit 2500));
  };

  audit.entities.ticket = {
    operations = [ "create" "update" "delete" "confirm" "cancel" "use" "upgrade" ];
    category = "workflow";
    reason = "Tickets represent customer entitlements and admission state.";
  };

  workflows.groups.ticketLifecycle = {
    label = "Ticket lifecycle";
    description = "Customer and staff operations that move a ticket from reservation through admission or cancellation.";
    displayPriority = 20;
  };

  operations.ticket = {
    confirm = {
      policy = {
        label = "Confirm ticket";
        description = "Customer or user-bound service confirms a reserved ticket.";
        audiences = [ "customer" "service" ];
      };
      workflow = {
        group = "ticketLifecycle";
        transitions = [{
          field = F.status;
          from = [ V.status.reserved ];
          to = V.status.confirmed;
        }];
        audit.summary = "Confirmed ticket";
      };
      guard = E.and
        (E.eq (E.f F.status) (E.lit V.status.reserved))
        # Can't confirm if performance is cancelled
        (E.ne (E.rel performance config.refs.performance.status) (E.lit performanceStatus.cancelled));
      set = [
        { field = F.status; value = V.status.confirmed; }
      ];
      effects = [
        { notify = { channel = "email"; template = "ticket_confirmation"; }; }
      ];
    };

    cancel = {
      policy = {
        label = "Cancel ticket";
        audiences = [ "customer" "service" ];
      };
      workflow = {
        group = "ticketLifecycle";
        transitions = [{
          field = F.status;
          from = with V.status; [ reserved confirmed ];
          to = V.status.cancelled;
        }];
        audit.summary = "Cancelled ticket";
        confirmation = {
          required = true;
          title = "Cancel ticket";
          message = "This will cancel the selected ticket.";
          confirmLabel = "Cancel ticket";
          severity = "warning";
        };
      };
      guard = E.or
        (E.eq (E.f F.status) (E.lit V.status.reserved))
        (E.eq (E.f F.status) (E.lit V.status.confirmed));
      set = [
        { field = F.status; value = V.status.cancelled; }
      ];
    };

    use = {
      relationships = [];
      audit = {
        required = true;
        category = "workflow";
        reason = "Ticket admission must be traceable for venue operations.";
      };
      policy = {
        label = "Use ticket";
        description = "Staff check-in operation for admitting a confirmed ticket.";
        audiences = [ "staff" "service" ];
      };
      workflow = {
        group = "ticketLifecycle";
        transitions = [{
          field = F.status;
          from = [ V.status.confirmed ];
          to = V.status.used;
        }];
        audit.summary = "Marked ticket as used";
      };
      guard = E.and
        (E.eq (E.f F.status) (E.lit V.status.confirmed))
        (E.eq (E.rel performance config.refs.performance.status) (E.lit performanceStatus.scheduled));
      set = [
        { field = F.status; value = V.status.used; }
      ];
    };

    upgrade = {
      audit = {
        required = true;
        category = "payment";
        reason = "Ticket upgrades can affect customer entitlement and price.";
      };
      policy = {
        label = "Upgrade ticket";
        audiences = [ "customer" "service" ];
      };
      workflow = {
        group = "ticketLifecycle";
        transitions = [{
          field = F.ticket_type;
          from = [ V.ticket_type.standard ];
          to = V.ticket_type.vip;
        }];
        audit.summary = "Upgraded ticket";
      };
      guard = E.and
        (E.eq (E.f F.ticket_type) (E.lit V.ticket_type.standard))
        (E.eq (E.f F.status) (E.lit V.status.confirmed));
      set = [
        { field = F.ticket_type; value = V.ticket_type.vip; }
      ];
    };
  };
}
