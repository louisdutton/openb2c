# Constraints

OpenB2C uses one constraint model and allocates enforcement to the database, application code, or both.

## Field Constraints

Single-field constraints are colocated with the field:

```nix
tables.performance.status = {
  type = "text";
  constraints.enum = [ "scheduled" "cancelled" "completed" ];
};

tables.ticket.price_pence.constraints.range.min = 0;
tables.user.email.constraints.pattern = "^[^@]+@[^@]+$";
```

Supported field constraints:

- `constraints.enum`
- `constraints.pattern`
- `constraints.length.min`
- `constraints.length.max`
- `constraints.range.min`
- `constraints.range.max`

## Record Constraints

Multi-field constraints live under `constraints.<entity>`:

```nix
constraints.rehearsal.check.ends_after_start = {
  fields = with config.refs.rehearsal; [
    starts_at
    ends_at
  ];
  expression = E.lt (E.f config.refs.rehearsal.starts_at) (E.f config.refs.rehearsal.ends_at);
};

constraints.ticket.unique.unique_performance_seat = {
  fields = with config.refs.ticket; [
    performance_id
    seat
  ];
};
```

Do not add hand-written message strings by default. Generated services and clients can derive readable constraint errors from the entity, fields, and constraint kind.
