# Operation Semantics

Operations separate business semantics from generated-client presentation.

## Semantic Fields

- `guard`: runtime precondition.
- `transitions`: state movement the operation performs.
- `set`: typed field assignments.
- `cascade`: typed assignments to related entities.
- `effects`: declared side effects referenced through `config.effectRefs`.
- `audit`: traceability requirements.
- `policy`: access audience and risk metadata.

```nix
operations.ticket.confirm = {
  guard = E.eq (E.f F.status) (E.lit V.status.reserved);
  transitions = [{
    field = F.status;
    from = [ V.status.reserved ];
    to = V.status.confirmed;
  }];
  set.status.value = V.status.confirmed;
  audit.summary = "Confirmed ticket";
  effects = [
    { notify = { template = ER.notifications.templates.ticket_confirmation; to = ER.notifications.recipients.customer; }; }
  ];
};
```

Typed assignments support exactly one source per field:

- `value`
- `now`
- `expression`
- `input`
- `clear`

## Presentation

Presentation hints live under `presentation`, not under workflow semantics:

```nix
presentation.groups.ticketLifecycle = {
  label = "Ticket lifecycle";
  displayPriority = 20;
};

operations.ticket.cancel.presentation = {
  label = "Cancel";
  group = "ticketLifecycle";
  confirmation.required = true;
  confirmation.severity = "danger";
};
```

Generated OpenAPI and UI metadata expose transitions, audit, policy, and presentation separately. Guards still decide whether an operation is allowed for a specific record at runtime.
