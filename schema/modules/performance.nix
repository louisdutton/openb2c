{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.performance;
  V = config.values.performance;
  ticket = config.refs.ticket;
  ticketStatus = config.values.ticket.status;
in
{
  tables.performance = {
    id = { type = "integer"; pk = true; auto = true; };
    venue_id = {
      type = "integer";
      required = true;
      references = "venue(id)";
      metadata = {
        label = "Venue";
        displayPriority = 20;
      };
      relationship = {
        label = "Venue";
        description = "Venue hosting this performance.";
      };
    };
    starts_at = {
      type = "text";
      required = true;
      metadata = {
        label = "Starts";
        placeholder = "YYYY-MM-DDTHH:MM";
        format = "date-time";
        displayPriority = 30;
      };
    };
    ends_at = {
      type = "text";
      required = true;
      metadata = {
        label = "Ends";
        placeholder = "YYYY-MM-DDTHH:MM";
        format = "date-time";
        displayPriority = 40;
      };
    };
    price_pence = {
      type = "integer";
      required = true;
      metadata = {
        label = "Price";
        format = "money";
        displayPriority = 50;
      };
      constraints.range.min = 0;
    };
    description = {
      type = "text";
      metadata = {
        label = "Description";
        format = "textarea";
        displayPriority = 60;
      };
    };
    status = {
      type = "text";
      default = "'scheduled'";
      metadata = {
        label = "Status";
        displayPriority = 70;
      };
      constraints.enum = [ "scheduled" "cancelled" "completed" ];
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

  constraints.performance.check.ends_after_start = {
    fields = with config.refs.performance; [
      starts_at
      ends_at
    ];
    expression = E.lt (E.f F.starts_at) (E.f F.ends_at);
  };

  audit.entities.performance = {
    operations = [ "create" "update" "delete" "cancel" "complete" "reschedule" ];
    category = "workflow";
    reason = "Performance changes affect public catalog state and ticket holder communications.";
  };

  # Junction table for performance artist participation.
  tables.performance_artist = {
    id = { type = "integer"; pk = true; auto = true; };
    performance_id = {
      type = "integer";
      required = true;
      references = "performance(id)";
      relationship = {
        label = "Performance";
      };
    };
    user_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      relationship = {
        label = "Artist";
      };
    };
    role_in_performance = { type = "text"; };  # Character name or role
  };

  constraints.performance_artist.unique.unique_pair = {
    fields = with config.refs.performance_artist; [
      performance_id
      user_id
    ];
  };

  workflows.groups.performanceLifecycle = {
    label = "Performance lifecycle";
    description = "Administrative operations for scheduled performances.";
    displayPriority = 10;
  };

  operations.performance = {
    read = {
      public = true;
      policy = {
        label = "Browse performances";
        description = "Public catalog access to scheduled performances.";
        audiences = [ "anonymous" "customer" ];
        risk = "low";
      };
    };

    cancel = {
      audit = {
        required = true;
        category = "workflow";
        reason = "Performance cancellation cascades to ticket state and customer notifications.";
      };
      policy = {
        label = "Cancel performance";
        description = "Administrative operation that cancels a scheduled performance and cascades ticket cancellation.";
        audiences = [ "staff" ];
        risk = "high";
      };
      workflow = {
        group = "performanceLifecycle";
        transitions = [{
          field = F.status;
          from = [ V.status.scheduled ];
          to = V.status.cancelled;
        }];
        audit.summary = "Cancelled performance";
        audit.detail = "Cancelled the performance and cascaded cancellation to related tickets.";
        confirmation = {
          required = true;
          title = "Cancel performance";
          message = "This will cancel the performance and notify ticket holders.";
          confirmLabel = "Cancel performance";
          severity = "danger";
        };
      };
      guard = E.eq (E.f F.status) (E.lit V.status.scheduled);
      set = [
        { field = F.status; value = V.status.cancelled; }
      ];
      cascade = [{
        entity = "ticket";
        via = "performance_id";
        set = [
          { field = ticket.status; value = ticketStatus.cancelled; }
        ];
      }];
      effects = [
        { notify = { channel = "email"; template = "performance_cancelled"; to = "ticket_holders"; }; }
      ];
    };

    complete = {
      policy = {
        label = "Complete performance";
        audiences = [ "staff" ];
      };
      workflow = {
        group = "performanceLifecycle";
        transitions = [{
          field = F.status;
          from = [ V.status.scheduled ];
          to = V.status.completed;
        }];
        audit.summary = "Completed performance";
      };
      guard = E.eq (E.f F.status) (E.lit V.status.scheduled);
      set = [
        { field = F.status; value = V.status.completed; }
      ];
    };

    reschedule = {
      audit = {
        required = true;
        category = "workflow";
        reason = "Performance rescheduling may notify ticket holders.";
      };
      policy = {
        label = "Reschedule performance";
        audiences = [ "staff" ];
        risk = "high";
      };
      workflow = {
        group = "performanceLifecycle";
        audit.summary = "Rescheduled performance";
        confirmation = {
          required = true;
          title = "Reschedule performance";
          message = "This may notify ticket holders about the changed start or end time.";
          confirmLabel = "Reschedule";
          severity = "warning";
        };
      };
      guard = E.eq (E.f F.status) (E.lit V.status.scheduled);
      # Note: actual start/end datetimes come from input, not hardcoded.
      set = {};  # Fields set from input
      effects = [
        { notify = { channel = "email"; template = "performance_rescheduled"; to = "ticket_holders"; }; }
      ];
    };
  };
  operations.performance_artist.read.public = true;
}
