{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  material = config.refs.production_material;
  materialStatus = config.values.production_material.status;
  version = config.refs.material_version;
  versionStatus = config.values.material_version.status;
  ER = config.effectRefs;
in
{
  tables.production_material = {
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
        description = "Production this material belongs to.";
      };
    };
    name = {
      type = "text";
      required = true;
      metadata = {
        label = "Material";
        placeholder = "Current vocal score";
        displayPriority = 20;
      };
      constraints.length.max = 160;
    };
    kind = {
      type = "text";
      default = "'score'";
      metadata = {
        label = "Kind";
        displayPriority = 30;
      };
      constraints.enum = [ "score" "script" "cut_list" "rehearsal_note" "briefing" "media" "other" ];
    };
    visibility = {
      type = "text";
      default = "'private'";
      metadata = {
        label = "Visibility";
        displayPriority = 40;
      };
      constraints.enum = [ "private" "participants" "public" ];
    };
    status = {
      type = "text";
      default = "'draft'";
      metadata = {
        label = "Status";
        displayPriority = 50;
      };
      constraints.enum = [ "draft" "current" "archived" ];
    };
    description = {
      type = "text";
      metadata = {
        label = "Description";
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

  tables.material_version = {
    id = { type = "integer"; pk = true; auto = true; };
    material_id = {
      type = "integer";
      required = true;
      references = "production_material(id)";
      metadata = {
        label = "Material";
        displayPriority = 10;
      };
      relationship = {
        label = "Material";
        description = "Material this version belongs to.";
      };
    };
    version_label = {
      type = "text";
      required = true;
      metadata = {
        label = "Version";
        placeholder = "v1";
        displayPriority = 20;
      };
      constraints.length.max = 80;
    };
    storage_uri = {
      type = "text";
      required = true;
      metadata = {
        label = "Storage URI";
        privacy = "internal";
        displayPriority = 30;
      };
    };
    checksum = {
      type = "text";
      metadata = {
        label = "Checksum";
        privacy = "internal";
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
      constraints.enum = [ "draft" "current" "superseded" "archived" ];
    };
    notes = {
      type = "text";
      metadata = {
        label = "Notes";
        format = "textarea";
        displayPriority = 60;
      };
    };
    created_by_user_id = {
      type = "integer";
      references = "user(id)";
      metadata = {
        label = "Created by";
        displayPriority = 70;
      };
      relationship = {
        label = "Created by";
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
  };

  constraints.material_version.unique.unique_material_version = {
    fields = with config.refs.material_version; [
      material_id
      version_label
    ];
  };

  audit.entities.production_material = {
    operations = [ "create" "update" "delete" "publish" "archive" ];
    category = "workflow";
    reason = "Production materials determine the current script, score, and operational references.";
  };

  audit.entities.material_version = {
    operations = [ "create" "update" "delete" "mark_current" "supersede" "archive" ];
    category = "workflow";
    reason = "Material versions preserve the authoritative production document history.";
  };

  presentation.groups.materialsLifecycle = {
    label = "Materials lifecycle";
    description = "Publishing, superseding, and archiving production materials.";
    displayPriority = 70;
  };

  effectContracts.notifications = {
    templates.material_published = {};
    recipients.participants = {};
  };

  operations.production_material = {
    publish = {
      relationships = [];
      presentation = {
        label = "Publish material";
        group = "materialsLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = material.status;
        from = [ materialStatus.draft ];
        to = materialStatus.current;
      }];
      audit.summary = "Published production material";
      guard = E.eq (E.f material.status) (E.lit materialStatus.draft);
      set.status.value = materialStatus.current;
      effects = [
        { notify = { template = ER.notifications.templates.material_published; to = ER.notifications.recipients.participants; }; }
      ];
    };

    archive = {
      relationships = [];
      presentation = {
        label = "Archive material";
        group = "materialsLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = material.status;
        from = with materialStatus; [ draft current ];
        to = materialStatus.archived;
      }];
      audit.summary = "Archived production material";
      guard = E.ne (E.f material.status) (E.lit materialStatus.archived);
      set.status.value = materialStatus.archived;
    };
  };

  operations.material_version = {
    mark_current = {
      relationships = [];
      presentation = {
        label = "Mark version current";
        group = "materialsLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = version.status;
        from = [ versionStatus.draft ];
        to = versionStatus.current;
      }];
      audit.summary = "Marked material version current";
      guard = E.eq (E.f version.status) (E.lit versionStatus.draft);
      set.status.value = versionStatus.current;
    };

    supersede = {
      relationships = [];
      presentation = {
        label = "Supersede version";
        group = "materialsLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = version.status;
        from = [ versionStatus.current ];
        to = versionStatus.superseded;
      }];
      audit.summary = "Superseded material version";
      guard = E.eq (E.f version.status) (E.lit versionStatus.current);
      set.status.value = versionStatus.superseded;
    };

    archive = {
      relationships = [];
      presentation = {
        label = "Archive version";
        group = "materialsLifecycle";
      };
      policy = {
        audiences = [ "staff" "service" ];
      };
      transitions = [{
        field = version.status;
        from = with versionStatus; [ draft current superseded ];
        to = versionStatus.archived;
      }];
      audit.summary = "Archived material version";
      guard = E.ne (E.f version.status) (E.lit versionStatus.archived);
      set.status.value = versionStatus.archived;
    };
  };
}
