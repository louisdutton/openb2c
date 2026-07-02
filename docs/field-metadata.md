# Field Metadata

Field metadata describes presentation, ordering, format, and privacy. Constraints describe validity.

```nix
tables.performance.starts_at = {
  type = "text";
  required = true;
  metadata = {
    label = "Starts";
    format = "date-time";
    displayPriority = 30;
  };
};
```

Supported metadata:

- `label`: generated UI and API label.
- `helpText`: short help text.
- `placeholder`: generated input placeholder.
- `format`: semantic format such as `email`, `phone`, `postcode`, `date-time`, `url`, `money`, or `textarea`.
- `displayPriority`: lower values appear earlier.
- `privacy`: `public`, `internal`, `sensitive`, or `secret`.
- `redact`: remove from generated REST responses.

Generated OpenAPI, MCP metadata, forms, lists, and detail views consume this metadata. Single-field validity rules belong in `constraints` beside the same field.
