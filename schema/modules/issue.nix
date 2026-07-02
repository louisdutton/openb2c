{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.issue;
  V = config.values.issue;
  project = config.entityRefs.project;
  projectStatus = config.values.project.status;
in
{
  tables.issue = {
    id = { type = "integer"; pk = true; auto = true; };
    project_id = {
      type = "integer";
      required = true;
      references = "project(id)";
      relationship = {
        label = "Project";
      };
    };
    number = { type = "integer"; required = true; };  # Project-scoped issue number
    name = { type = "text"; required = true; };
    description = { type = "text"; };
    type = {
      type = "text";
      default = "'task'";
      constraints.enum = [ "task" "bug" "feature" "epic" ];
    };
    status = {
      type = "text";
      default = "'todo'";
      constraints.enum = [ "todo" "in_progress" "in_review" "done" "cancelled" ];
    };
    priority = {
      type = "text";
      default = "'medium'";
      constraints.enum = [ "low" "medium" "high" "urgent" ];
    };
    creator_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      relationship = {
        label = "Creator";
      };
    };
    assignee_id = {
      type = "integer";
      references = "user(id)";
      relationship = {
        label = "Assignee";
      };
    };
    created_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
    updated_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
  };

  workflows.groups.issueWorkflow = {
    label = "Issue workflow";
    description = "Issue progress, review, completion, and escalation operations.";
    displayPriority = 40;
  };

  operations.issue = {
    read.relationships = [ "creator" "assignee" ];
    create.relationships = [ "creator" ];
    update.relationships = [ "creator" "assignee" ];

    start = {
      workflow = {
        group = "issueWorkflow";
        transitions = [{
          field = F.status;
          from = [ V.status.todo ];
          to = V.status.in_progress;
        }];
        audit.summary = "Started issue";
      };
      guard = E.and
        (E.and
          (E.eq (E.f F.status) (E.lit V.status.todo))
          (E.notNull (E.f F.assignee_id)))
        (E.eq (E.rel project config.refs.project.status) (E.lit projectStatus.active));
      set = [
        { field = F.status; value = V.status.in_progress; }
      ];
      effects = [
        { notify = { channel = "email"; template = "issue_started"; }; }
      ];
    };

    submit_for_review = {
      workflow = {
        group = "issueWorkflow";
        transitions = [{
          field = F.status;
          from = [ V.status.in_progress ];
          to = V.status.in_review;
        }];
        audit.summary = "Submitted issue for review";
      };
      guard = E.eq (E.f F.status) (E.lit V.status.in_progress);
      set = [
        { field = F.status; value = V.status.in_review; }
      ];
      effects = [
        { notify = { channel = "email"; template = "issue_review_requested"; }; }
      ];
    };

    complete = {
      workflow = {
        group = "issueWorkflow";
        transitions = [{
          field = F.status;
          from = with V.status; [ in_review in_progress ];
          to = V.status.done;
        }];
        audit.summary = "Completed issue";
      };
      guard = E.or
        (E.eq (E.f F.status) (E.lit V.status.in_review))
        (E.eq (E.f F.status) (E.lit V.status.in_progress));
      set = [
        { field = F.status; value = V.status.done; }
      ];
      effects = [
        { emit = "issue.completed"; }
      ];
    };

    cancel = {
      workflow = {
        group = "issueWorkflow";
        transitions = [{
          field = F.status;
          from = with V.status; [ todo in_progress in_review ];
          to = V.status.cancelled;
        }];
        audit.summary = "Cancelled issue";
        confirmation = {
          required = true;
          title = "Cancel issue";
          message = "This will stop active work on the issue.";
          confirmLabel = "Cancel issue";
          severity = "warning";
        };
      };
      guard = E.and
        (E.ne (E.f F.status) (E.lit V.status.done))
        (E.ne (E.f F.status) (E.lit V.status.cancelled));
      set = [
        { field = F.status; value = V.status.cancelled; }
      ];
    };

    reopen = {
      workflow = {
        group = "issueWorkflow";
        transitions = [{
          field = F.status;
          from = with V.status; [ done cancelled ];
          to = V.status.todo;
        }];
        audit.summary = "Reopened issue";
      };
      guard = E.or
        (E.eq (E.f F.status) (E.lit V.status.done))
        (E.eq (E.f F.status) (E.lit V.status.cancelled));
      set = [
        { field = F.status; value = V.status.todo; }
      ];
    };

    assign = {
      relationships = [];
      policy = {
        label = "Assign issue";
        description = "Staff workflow operation for changing issue ownership.";
        audiences = [ "staff" ];
      };
      workflow = {
        group = "issueWorkflow";
        audit.summary = "Assigned issue";
      };
      guard = E.and
        (E.ne (E.f F.status) (E.lit V.status.done))
        (E.ne (E.f F.status) (E.lit V.status.cancelled));
      set = { };  # assignee_id set via parameters
      effects = [
        { notify = { channel = "email"; template = "issue_assigned"; }; }
      ];
    };

    escalate = {
      relationships = [];
      policy = {
        label = "Escalate issue";
        audiences = [ "staff" ];
        risk = "high";
      };
      workflow = {
        group = "issueWorkflow";
        audit.summary = "Escalated issue";
        confirmation = {
          required = true;
          title = "Escalate issue";
          message = "This will mark the issue as urgent.";
          confirmLabel = "Escalate";
          severity = "warning";
        };
      };
      guard = E.and
        (E.ne (E.f F.priority) (E.lit V.priority.urgent))
        (E.ne (E.f F.status) (E.lit V.status.done));
      set = [
        { field = F.priority; value = V.priority.urgent; }
      ];
      effects = [
        { notify = { channel = "email"; template = "issue_escalated"; }; }
      ];
    };
  };
}
