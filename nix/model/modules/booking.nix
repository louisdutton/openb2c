{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.booking;
  V = config.values.booking;
  ER = config.effectRefs;
  ticket = config.refs.ticket;
  ticketStatus = config.values.ticket.status;
in
{
  tables.booking = {
    id = { type = "integer"; pk = true; auto = true; };
    user_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      relationship = {
        label = "Customer";
        description = "Customer who owns this checkout.";
      };
    };
    status = {
      type = "text";
      default = "'checkout_pending'";
      constraints.enum = [
        "checkout_pending"
        "payment_pending"
        "paid"
        "expired"
        "cancelled"
        "refund_required"
        "refunded"
      ];
    };
    amount_pence = { type = "integer"; required = true; };
    currency = { type = "text"; default = "'GBP'"; };
    expires_at = { type = "text"; required = true; };
    payment_deadline_at = { type = "text"; };
    payment_reference = { type = "text"; };
    client = { type = "text"; default = "'web'"; };
    created_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
    updated_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
  };

  tables.booking_ticket = {
    id = { type = "integer"; pk = true; auto = true; };
    booking_id = {
      type = "integer";
      required = true;
      references = "booking(id)";
      relationship = {
        label = "Booking";
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

  constraints.booking.unique.unique_payment_reference = {
    fields = with config.refs.booking; [ payment_reference ];
  };

  constraints.booking_ticket.unique.unique_ticket = {
    fields = with config.refs.booking_ticket; [ ticket_id ];
  };

  constraints.booking_ticket.unique.unique_pair = {
    fields = with config.refs.booking_ticket; [
      booking_id
      ticket_id
    ];
  };

  effectContracts.notifications = {
    templates.booking_cancelled = {};
    recipients.customer = {};
  };

  operations.booking = {
    cancel = {
      guard = E.or
        (E.eq (E.f F.status) (E.lit V.status.checkout_pending))
        (E.eq (E.f F.status) (E.lit V.status.payment_pending));
      set.status.value = V.status.cancelled;
      cascade = [{
        entity = config.entityRefs.ticket;
        via = config.entityRefs.booking_ticket;
        set.status.value = ticketStatus.cancelled;
      }];
      effects = [
        { notify = { template = ER.notifications.templates.booking_cancelled; to = ER.notifications.recipients.customer; }; }
      ];
    };
  };
}
