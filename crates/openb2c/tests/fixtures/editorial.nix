# Application-owned workflow: no platform imports or generated Rust edits.
{
  config,
  expr,
  ...
}: {
  name = "Publisher editorial workflow";
  description = "An independently maintained business module";
  currency = "GBP";
  tables.editorial_job = {
    id = {
      type = "integer";
      pk = true;
      auto = true;
    };
    title = {
      type = "text";
      required = true;
    };
    status = {
      type = "text";
      required = true;
      default = "'draft'";
      constraints.enum = ["draft" "approved"];
    };
  };
  operations.editorial_job = {
    read.scope = "editorial.read";
    create = {
      scope = "editorial.write";
      set.status.value = config.values.editorial_job.status.draft;
    };
    update.enabled = false;
    delete.enabled = false;
    approve = {
      scope = "editorial.approve";
      guard = expr.eq (expr.f config.refs.editorial_job.status) (expr.lit config.values.editorial_job.status.draft);
      set.status.value = config.values.editorial_job.status.approved;
    };
  };
}
