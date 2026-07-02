# Relationship Metadata

Foreign keys are the structural source of truth.

```nix
tables.ticket.performance_id = {
  type = "integer";
  required = true;
  references = "performance(id)";
  relationship = {
    label = "Performance";
    description = "Performance this ticket admits the customer to.";
  };
};
```

`relationship` is only valid on FK fields and may only describe presentation semantics such as label, description, and cardinality. It must not define structure that is absent from `references`.

Generated clients use the FK graph to infer workspaces, related collections, selectors, matrices, schedules, and profile extensions. A one-to-one required unique FK such as `artist_profile.user_id -> user(id)` is inferred as a profile extension automatically.

Authorization relationships are separate. They describe whether `auth.userId` is allowed to act on a record:

```nix
operations.ticket.read.relationships = [ "user" ];
relationships.issue.reviewer.field = config.refs.issue.reviewed_by_user_id;
```

Relationship names resolve by convention to user FKs such as `user_id`, `creator_id`, or `assignee_id`; explicit relationship definitions are only needed when convention is not enough.
