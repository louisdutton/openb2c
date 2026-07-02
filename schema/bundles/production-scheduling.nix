{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  production = config.refs.production;
  productionStatus = config.values.production.status;
  rehearsal = config.refs.rehearsal;
  rehearsalStatus = config.values.rehearsal.status;
  rehearsalCall = config.refs.rehearsal_call;
  callStatus = config.values.rehearsal_call.call_status;
  requirement = config.refs.rehearsal_requirement;
  requirementStatus = config.values.rehearsal_requirement.status;
  coverage = config.refs.rehearsal_coverage;
  coverageStatus = config.values.rehearsal_coverage.status;
  ER = config.effectRefs;
in
{
  tables.production = {
    id = { type = "integer"; pk = true; auto = true; };
    name = {
      type = "text";
      required = true;
      metadata = {
        label = "Production";
        placeholder = "The Magic Flute";
        displayPriority = 10;
      };
      constraints.length.max = 160;
    };
    season = {
      type = "text";
      metadata = {
        label = "Season";
        placeholder = "Summer 2026";
        displayPriority = 20;
      };
      constraints.length.max = 80;
    };
    description = {
      type = "text";
      metadata = {
        label = "Description";
        format = "textarea";
        displayPriority = 30;
      };
    };
    status = {
      type = "text";
      default = "'planning'";
      metadata = {
        label = "Status";
        displayPriority = 40;
      };
      constraints.enum = [ "planning" "active" "completed" "cancelled" ];
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
    updated_at = {
      type = "text";
      default = "CURRENT_TIMESTAMP";
      metadata = {
        label = "Updated";
        format = "date-time";
        displayPriority = 1010;
      };
    };
  };

  tables.performance.production_id = {
    type = "integer";
    required = true;
    references = "production(id)";
    metadata = {
      label = "Production";
      displayPriority = 15;
    };
    relationship = {
      label = "Production";
      description = "Production this performance belongs to.";
    };
  };

  tables.production_role = {
    id = { type = "integer"; pk = true; auto = true; };
    production_id = {
      type = "integer";
      required = true;
      references = "production(id)";
      metadata = {
        label = "Production";
        displayPriority = 10;
      };
      relationship = {
        label = "Production";
        description = "Production this role belongs to.";
      };
    };
    name = {
      type = "text";
      required = true;
      metadata = {
        label = "Role";
        placeholder = "Pamina";
        displayPriority = 20;
      };
      constraints.length.max = 120;
    };
    category = {
      type = "text";
      default = "'cast'";
      metadata = {
        label = "Category";
        displayPriority = 30;
      };
      constraints.enum = [ "cast" "creative" "crew" "orchestra" "admin" ];
    };
    description = {
      type = "text";
      metadata = {
        label = "Description";
        format = "textarea";
        displayPriority = 40;
      };
    };
    required_count = {
      type = "integer";
      default = "1";
      metadata = {
        label = "Required count";
        displayPriority = 50;
      };
      constraints.range.min = 1;
    };
    status = {
      type = "text";
      default = "'open'";
      metadata = {
        label = "Status";
        displayPriority = 60;
      };
      constraints.enum = [ "open" "filled" "withdrawn" ];
    };
  };

  tables.production_member = {
    id = { type = "integer"; pk = true; auto = true; };
    production_id = {
      type = "integer";
      required = true;
      references = "production(id)";
      relationship = {
        label = "Production";
      };
    };
    user_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      relationship = {
        label = "Person";
        description = "Person or performer participating in the production.";
      };
    };
    role_id = {
      type = "integer";
      references = "production_role(id)";
      relationship = {
        label = "Role";
      };
    };
    responsibility = {
      type = "text";
      metadata = {
        label = "Responsibility";
        placeholder = "Soprano";
        displayPriority = 30;
      };
    };
    status = {
      type = "text";
      default = "'confirmed'";
      metadata = {
        label = "Status";
        displayPriority = 40;
      };
      constraints.enum = [ "invited" "confirmed" "declined" "released" ];
    };
  };

  tables.rehearsal = {
    id = { type = "integer"; pk = true; auto = true; };
    production_id = {
      type = "integer";
      required = true;
      references = "production(id)";
      relationship = {
        label = "Production";
      };
    };
    name = {
      type = "text";
      required = true;
      metadata = {
        label = "Rehearsal";
        placeholder = "Act I music call";
        displayPriority = 10;
      };
      constraints.length.max = 160;
    };
    venue_id = {
      type = "integer";
      references = "venue(id)";
      metadata = {
        label = "Venue";
        displayPriority = 20;
      };
      relationship = {
        label = "Venue";
        description = "Venue or room reserved for this rehearsal.";
      };
    };
    starts_at = {
      type = "text";
      required = true;
      metadata = {
        label = "Starts";
        format = "date-time";
        displayPriority = 30;
      };
    };
    ends_at = {
      type = "text";
      required = true;
      metadata = {
        label = "Ends";
        format = "date-time";
        displayPriority = 40;
      };
    };
    status = {
      type = "text";
      default = "'draft'";
      metadata = {
        label = "Status";
        displayPriority = 50;
      };
      constraints.enum = [ "draft" "published" "completed" "cancelled" ];
    };
    notes = {
      type = "text";
      metadata = {
        label = "Notes";
        format = "textarea";
        displayPriority = 60;
      };
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
    updated_at = {
      type = "text";
      default = "CURRENT_TIMESTAMP";
      metadata = {
        label = "Updated";
        format = "date-time";
        displayPriority = 1010;
      };
    };
  };

  tables.rehearsal_call = {
    id = { type = "integer"; pk = true; auto = true; };
    rehearsal_id = {
      type = "integer";
      required = true;
      references = "rehearsal(id)";
      relationship = {
        label = "Rehearsal";
      };
    };
    user_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      relationship = {
        label = "Participant";
      };
    };
    role_id = {
      type = "integer";
      references = "production_role(id)";
      relationship = {
        label = "Role";
      };
    };
    call_status = {
      type = "text";
      default = "'called'";
      metadata = {
        label = "Status";
        displayPriority = 30;
      };
      constraints.enum = [ "called" "confirmed" "declined" "attended" "absent" ];
    };
    notes = {
      type = "text";
      metadata = {
        label = "Notes";
        format = "textarea";
        displayPriority = 40;
      };
    };
  };

  tables.rehearsal_requirement = {
    id = { type = "integer"; pk = true; auto = true; };
    production_id = {
      type = "integer";
      required = true;
      references = "production(id)";
      relationship = {
        label = "Production";
      };
    };
    name = {
      type = "text";
      required = true;
      metadata = {
        label = "Requirement";
        placeholder = "Act I finale";
        displayPriority = 10;
      };
      constraints.length.max = 160;
    };
    category = {
      type = "text";
      default = "'scene'";
      metadata = {
        label = "Category";
        displayPriority = 20;
      };
      constraints.enum = [ "scene" "act" "role" "ensemble" "technical" "music" ];
    };
    required_sessions = {
      type = "integer";
      default = "1";
      metadata = {
        label = "Required sessions";
        displayPriority = 30;
      };
      constraints.range.min = 1;
    };
    status = {
      type = "text";
      default = "'open'";
      metadata = {
        label = "Status";
        displayPriority = 40;
      };
      constraints.enum = [ "open" "scheduled" "covered" "dropped" ];
    };
  };

  tables.rehearsal_coverage = {
    id = { type = "integer"; pk = true; auto = true; };
    requirement_id = {
      type = "integer";
      required = true;
      references = "rehearsal_requirement(id)";
      relationship = {
        label = "Requirement";
      };
    };
    rehearsal_id = {
      type = "integer";
      required = true;
      references = "rehearsal(id)";
      relationship = {
        label = "Rehearsal";
      };
    };
    status = {
      type = "text";
      default = "'planned'";
      metadata = {
        label = "Status";
        displayPriority = 30;
      };
      constraints.enum = [ "planned" "covered" "deferred" ];
    };
    notes = {
      type = "text";
      metadata = {
        label = "Notes";
        format = "textarea";
        displayPriority = 40;
      };
    };
  };

  constraints.production_member.unique.unique_production_user = {
    fields = with config.refs.production_member; [
      production_id
      user_id
    ];
  };

  constraints.rehearsal_call.unique.unique_rehearsal_user = {
    fields = with config.refs.rehearsal_call; [
      rehearsal_id
      user_id
    ];
  };

  constraints.rehearsal_coverage.unique.unique_requirement_rehearsal = {
    fields = with config.refs.rehearsal_coverage; [
      requirement_id
      rehearsal_id
    ];
  };

  constraints.rehearsal.check.ends_after_start = {
    fields = with config.refs.rehearsal; [
      starts_at
      ends_at
    ];
    expression = E.lt (E.f rehearsal.starts_at) (E.f rehearsal.ends_at);
  };

  audit.entities.production = {
    operations = [ "create" "update" "delete" "activate" "complete" "cancel" ];
    category = "workflow";
    reason = "Production state controls private scheduling, calls, and materials.";
  };

  audit.entities.rehearsal = {
    operations = [ "create" "update" "delete" "publish" "complete" "cancel" ];
    category = "workflow";
    reason = "Rehearsal changes affect participant availability and notifications.";
  };

  audit.entities.rehearsal_call = {
    operations = [ "create" "update" "delete" "confirm" "decline" "mark_attended" "mark_absent" ];
    category = "workflow";
    reason = "Rehearsal calls record participant commitments and attendance.";
  };

  audit.entities.rehearsal_requirement = {
    operations = [ "create" "update" "delete" "schedule" "cover" "drop" ];
    category = "workflow";
    reason = "Coverage requirements track whether a production schedule is complete.";
  };

  presentation.groups.productionLifecycle = {
    label = "Production lifecycle";
    description = "Administrative lifecycle for private production planning.";
    displayPriority = 30;
  };

  presentation.groups.rehearsalLifecycle = {
    label = "Rehearsal lifecycle";
    description = "Publishing, completing, and cancelling rehearsal sessions.";
    displayPriority = 40;
  };

  presentation.groups.rehearsalAttendance = {
    label = "Rehearsal attendance";
    description = "Participant call responses and attendance recording.";
    displayPriority = 50;
  };

  presentation.groups.rehearsalCoverage = {
    label = "Rehearsal coverage";
    description = "Coverage tracking for scenes, roles, music, and technical requirements.";
    displayPriority = 60;
  };

  effectContracts.notifications = {
    templates = {
      rehearsal_published = {};
      rehearsal_cancelled = {};
    };
    recipients.participants = {};
  };

  operations.production = {
    activate = {
      relationships = [];
      presentation = {
        label = "Activate production";
        group = "productionLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = production.status;
        from = [ productionStatus.planning ];
        to = productionStatus.active;
      }];
      audit.summary = "Activated production";
      guard = E.eq (E.f production.status) (E.lit productionStatus.planning);
      set.status.value = productionStatus.active;
    };

    complete = {
      relationships = [];
      presentation = {
        label = "Complete production";
        group = "productionLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = production.status;
        from = [ productionStatus.active ];
        to = productionStatus.completed;
      }];
      audit.summary = "Completed production";
      guard = E.eq (E.f production.status) (E.lit productionStatus.active);
      set.status.value = productionStatus.completed;
    };

    cancel = {
      relationships = [];
      presentation = {
        label = "Cancel production";
        group = "productionLifecycle";
        confirmation = {
          required = true;
          title = "Cancel production";
          message = "This will cancel the production for internal scheduling.";
          confirmLabel = "Cancel production";
          severity = "danger";
        };
      };
      policy = {
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      transitions = [{
        field = production.status;
        from = with productionStatus; [ planning active ];
        to = productionStatus.cancelled;
      }];
      audit.summary = "Cancelled production";
      guard = E.and
        (E.ne (E.f production.status) (E.lit productionStatus.completed))
        (E.ne (E.f production.status) (E.lit productionStatus.cancelled));
      set.status.value = productionStatus.cancelled;
    };
  };

  operations.rehearsal = {
    publish = {
      relationships = [];
      presentation = {
        label = "Publish rehearsal";
        group = "rehearsalLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = rehearsal.status;
        from = [ rehearsalStatus.draft ];
        to = rehearsalStatus.published;
      }];
      audit.summary = "Published rehearsal";
      guard = E.eq (E.f rehearsal.status) (E.lit rehearsalStatus.draft);
      set.status.value = rehearsalStatus.published;
      effects = [
        { notify = { template = ER.notifications.templates.rehearsal_published; to = ER.notifications.recipients.participants; }; }
      ];
    };

    complete = {
      relationships = [];
      presentation = {
        label = "Complete rehearsal";
        group = "rehearsalLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = rehearsal.status;
        from = [ rehearsalStatus.published ];
        to = rehearsalStatus.completed;
      }];
      audit.summary = "Completed rehearsal";
      guard = E.eq (E.f rehearsal.status) (E.lit rehearsalStatus.published);
      set.status.value = rehearsalStatus.completed;
    };

    cancel = {
      relationships = [];
      presentation = {
        label = "Cancel rehearsal";
        group = "rehearsalLifecycle";
        confirmation = {
          required = true;
          title = "Cancel rehearsal";
          message = "This will cancel the rehearsal and notify participants.";
          confirmLabel = "Cancel rehearsal";
          severity = "warning";
        };
      };
      policy = {
        audiences = [ "staff" "service" ];
        risk = "high";
      };
      transitions = [{
        field = rehearsal.status;
        from = with rehearsalStatus; [ draft published ];
        to = rehearsalStatus.cancelled;
      }];
      audit.summary = "Cancelled rehearsal";
      guard = E.and
        (E.ne (E.f rehearsal.status) (E.lit rehearsalStatus.completed))
        (E.ne (E.f rehearsal.status) (E.lit rehearsalStatus.cancelled));
      set.status.value = rehearsalStatus.cancelled;
      effects = [
        { notify = { template = ER.notifications.templates.rehearsal_cancelled; to = ER.notifications.recipients.participants; }; }
      ];
    };
  };

  operations.rehearsal_call = {
    confirm = {
      relationships = [];
      presentation = {
        label = "Confirm call";
        group = "rehearsalAttendance";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = rehearsalCall.call_status;
        from = [ callStatus.called ];
        to = callStatus.confirmed;
      }];
      audit.summary = "Confirmed rehearsal call";
      guard = E.eq (E.f rehearsalCall.call_status) (E.lit callStatus.called);
      set.call_status.value = callStatus.confirmed;
    };

    decline = {
      relationships = [];
      presentation = {
        label = "Decline call";
        group = "rehearsalAttendance";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = rehearsalCall.call_status;
        from = with callStatus; [ called confirmed ];
        to = callStatus.declined;
      }];
      audit.summary = "Declined rehearsal call";
      guard = E.or
        (E.eq (E.f rehearsalCall.call_status) (E.lit callStatus.called))
        (E.eq (E.f rehearsalCall.call_status) (E.lit callStatus.confirmed));
      set.call_status.value = callStatus.declined;
    };

    mark_attended = {
      relationships = [];
      presentation = {
        label = "Mark attended";
        group = "rehearsalAttendance";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = rehearsalCall.call_status;
        from = [ callStatus.confirmed ];
        to = callStatus.attended;
      }];
      audit.summary = "Marked rehearsal call attended";
      guard = E.eq (E.f rehearsalCall.call_status) (E.lit callStatus.confirmed);
      set.call_status.value = callStatus.attended;
    };

    mark_absent = {
      relationships = [];
      presentation = {
        label = "Mark absent";
        group = "rehearsalAttendance";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = rehearsalCall.call_status;
        from = [ callStatus.confirmed ];
        to = callStatus.absent;
      }];
      audit.summary = "Marked rehearsal call absent";
      guard = E.eq (E.f rehearsalCall.call_status) (E.lit callStatus.confirmed);
      set.call_status.value = callStatus.absent;
    };
  };

  operations.rehearsal_requirement = {
    schedule = {
      relationships = [];
      presentation = {
        label = "Schedule requirement";
        group = "rehearsalCoverage";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = requirement.status;
        from = [ requirementStatus.open ];
        to = requirementStatus.scheduled;
      }];
      audit.summary = "Scheduled rehearsal requirement";
      guard = E.eq (E.f requirement.status) (E.lit requirementStatus.open);
      set.status.value = requirementStatus.scheduled;
    };

    cover = {
      relationships = [];
      presentation = {
        label = "Cover requirement";
        group = "rehearsalCoverage";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = requirement.status;
        from = with requirementStatus; [ open scheduled ];
        to = requirementStatus.covered;
      }];
      audit.summary = "Covered rehearsal requirement";
      guard = E.or
        (E.eq (E.f requirement.status) (E.lit requirementStatus.open))
        (E.eq (E.f requirement.status) (E.lit requirementStatus.scheduled));
      set.status.value = requirementStatus.covered;
    };

    drop = {
      relationships = [];
      presentation = {
        label = "Drop requirement";
        group = "rehearsalCoverage";
        confirmation = {
          required = true;
          title = "Drop requirement";
          message = "This removes the requirement from coverage tracking.";
          confirmLabel = "Drop requirement";
          severity = "warning";
        };
      };
      policy = {
        audiences = [ "staff" "service" ];
        risk = "medium";
      };
      transitions = [{
        field = requirement.status;
        from = with requirementStatus; [ open scheduled ];
        to = requirementStatus.dropped;
      }];
      audit.summary = "Dropped rehearsal requirement";
      guard = E.and
        (E.ne (E.f requirement.status) (E.lit requirementStatus.covered))
        (E.ne (E.f requirement.status) (E.lit requirementStatus.dropped));
      set.status.value = requirementStatus.dropped;
    };
  };

  operations.rehearsal_coverage = {
    mark_covered = {
      relationships = [];
      presentation = {
        label = "Mark covered";
        group = "rehearsalCoverage";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = coverage.status;
        from = with coverageStatus; [ planned deferred ];
        to = coverageStatus.covered;
      }];
      audit.summary = "Marked requirement coverage complete";
      guard = E.or
        (E.eq (E.f coverage.status) (E.lit coverageStatus.planned))
        (E.eq (E.f coverage.status) (E.lit coverageStatus.deferred));
      set.status.value = coverageStatus.covered;
    };

    defer = {
      relationships = [];
      presentation = {
        label = "Defer coverage";
        group = "rehearsalCoverage";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = coverage.status;
        from = [ coverageStatus.planned ];
        to = coverageStatus.deferred;
      }];
      audit.summary = "Deferred requirement coverage";
      guard = E.eq (E.f coverage.status) (E.lit coverageStatus.planned);
      set.status.value = coverageStatus.deferred;
    };
  };
}
