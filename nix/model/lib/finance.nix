# Shared field definitions; capability modules remain authoritative owners of tables.
{
  id = {
    type = "integer";
    pk = true;
    auto = true;
  };
  text = {
    type = "text";
    required = true;
  };
  int = {
    type = "integer";
    required = true;
  };
  optionalInt = {type = "integer";};
  optionalText = {type = "text";};
  fk = table: {
    type = "integer";
    required = true;
    references = "${table}(id)";
  };
  enum = values: {
    type = "text";
    required = true;
    constraints.enum = values;
  };
}
