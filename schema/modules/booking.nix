{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
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
    status = { type = "text"; default = "'checkout_pending'"; };  # checkout_pending, paid, expired, cancelled
    amount_pence = { type = "integer"; required = true; };
    currency = { type = "text"; default = "'GBP'"; };
    expires_at = { type = "text"; required = true; };
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

  constraints.booking.unique_payment_reference = {
    kind = "unique";
    fields = [ config.refs.booking.payment_reference ];
    message = "Payment reference must be unique.";
  };

  constraints.booking_ticket.unique_ticket = {
    kind = "unique";
    fields = [ config.refs.booking_ticket.ticket_id ];
    message = "A ticket can only belong to one active booking.";
  };

  constraints.booking_ticket.unique_pair = {
    kind = "unique";
    fields = [
      config.refs.booking_ticket.booking_id
      config.refs.booking_ticket.ticket_id
    ];
    message = "A ticket can only appear once in the same booking.";
  };

  operations.booking = {
    cancel = {
      guard = E.eq (E.f "status") (E.lit "checkout_pending");
      set = { status = "cancelled"; };
      cascade = [{
        entity = "ticket";
        via = "booking_ticket";
        set = { status = "cancelled"; };
      }];
      effects = [
        { notify = { channel = "email"; template = "booking_cancelled"; }; }
      ];
    };
  };
}
