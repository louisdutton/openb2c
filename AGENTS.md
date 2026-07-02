# OpenB2C

OpenB2C is a declarative generator for composable B2C and operational systems.

The premise is that most organisations need the same core machinery: users, profiles, events, resources, documents, bookings, payments, permissions, notifications, audit, and admin workflows. OpenB2C lets a project compose those pieces in Nix and generates the backend APIs plus public/private web surfaces from that single model.

## Source Of Truth

The evaluated Nix schema is authoritative.

- Tables and foreign keys define the data graph.
- Foreign keys are the primary relationship signal.
- Generated UI is inferred from schema structure, operations, constraints, metadata, and temporal fields.
- Avoid parallel UI schemas unless inference genuinely cannot express the workflow.
- Generated code is output under each example's `generated/` directory and should not be hand-edited.

## Structure

```text
schema/
├── base.nix              # Nix option model
├── lib/                  # expression and composition helpers
├── modules/              # reusable entity/capability modules
├── bundles/              # opinionated cross-entity bundles
├── codegen/              # SQL, TS, REST, MCP, OpenAPI, UI generation
└── ui/                   # generated public/admin web component runtimes

examples/
└── duchyopera/           # current pilot app
```

## Schema Conventions

- Put single-field constraints beside the field: `constraints.enum`, `constraints.length`, `constraints.range`, `constraints.pattern`.
- Put multi-field constraints under `constraints.<entity>.unique` or `constraints.<entity>.check`.
- Use `references = "target(id)"` for structure. Field `relationship` metadata may only describe that FK for generated clients.
- Use `config.refs.<entity>.<field>` for field references.
- Use `config.values.<entity>.<field>.<value>` for enum values.
- Use `config.entityRefs.<entity>` for entity references.
- Use `config.effectRefs` for declared effects.

Operations separate semantics from presentation:

```nix
operations.ticket.confirm = {
  guard = E.eq (E.f F.status) (E.lit V.status.reserved);
  transitions = [{
    field = F.status;
    from = [ V.status.reserved ];
    to = V.status.confirmed;
  }];
  set.status.value = V.status.confirmed;
  presentation.group = "ticketLifecycle";
  audit.summary = "Confirmed ticket";
  effects = [
    { notify = { template = ER.notifications.templates.ticket_confirmation; to = ER.notifications.recipients.customer; }; }
  ];
};
```

Typed assignments support `value`, `now`, `expression`, `input`, and `clear`. Exactly one assignment source should be set.

## Commands

```bash
nix develop
compose examples/duchyopera/composition.nix
compose --explain examples/duchyopera/composition.nix
bun test
```

Run the Duchy Opera generated server:

```bash
cd examples/duchyopera
bun run server
```

## Current Focus

Duchy Opera is the acceptance case for proving the wider platform: public performance booking, ticket checkout, payments, and private production operations should all come from reusable schema modules and graph-based generation.
