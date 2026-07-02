{ config, lib, ... }:
let
  E = import ../lib/expr.nix;
  F = config.refs.project;
  V = config.values.project;
in
{
  tables.project = {
    id = { type = "integer"; pk = true; auto = true; };
    key = { type = "text"; required = true; };  # e.g., "PROJ", "ENG"
    name = { type = "text"; required = true; };
    description = { type = "text"; };
    owner_id = {
      type = "integer";
      required = true;
      references = "user(id)";
      relationship = {
        label = "Owner";
        description = "User accountable for this project.";
      };
    };
    status = {
      type = "text";
      default = "'active'";
      constraints.enum = [ "active" "archived" ];
    };
    created_at = { type = "text"; default = "CURRENT_TIMESTAMP"; };
  };

  constraints.project.unique.unique_key = {
    fields = with config.refs.project; [ key ];
  };

  operations.project = {
    read.relationships = [ "owner" ];
    create.relationships = [ "owner" ];
    update.relationships = [ "owner" ];

    archive = {
      guard = E.eq (E.f F.status) (E.lit V.status.active);
      set = [
        { field = F.status; value = V.status.archived; }
      ];
    };

    unarchive = {
      guard = E.eq (E.f F.status) (E.lit V.status.archived);
      set = [
        { field = F.status; value = V.status.active; }
      ];
    };
  };
}
