# Derived Fields

Derived fields are computed response values. They are not database columns and are not accepted in create or update inputs.

Use derived fields for real computed facts, not redundant presentation labels.

```nix
derived.booking.total_label = {
  type = "text";
  dependencies = [
    config.refs.booking.amount_pence
    config.refs.booking.currency
  ];
  template = "{currency} {amount_pence}";
};
```

Generated row types, response schemas, REST responses, and MCP outputs include derived fields. Input types exclude them.

Derived fields can use:

- `template` with `{field_name}` placeholders.
- `expression` using the local expression AST.

Codegen validates that dependencies reference stored fields on the same entity and that derived field names do not collide with real columns.
