# Service-managed entities expose owner-scoped reads, with no generic writes.
scope: {
  read = {
    inherit scope;
    relationships = ["user"];
  };
  create.enabled = false;
  update.enabled = false;
  delete.enabled = false;
}
