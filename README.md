# OpenB2C

OpenB2C is a declarative generator for composable B2C and operational systems.

Most organisations need the same underlying machinery: users, profiles, events, resources, documents, bookings, payments, permissions, notifications, audit, and admin workflows. OpenB2C provides those pieces as reusable Nix modules and bundles, then generates a working SQLite/Bun application from the evaluated composition.

## Problem

Bespoke business systems usually repeat the same schema, API, auth, admin, and checkout work. That creates fragile one-off apps and makes every new client feel like a greenfield build.

OpenB2C aims to make those common capabilities prebuilt and composable, while still allowing a specific organisation to assemble the combination it needs.

## Solution

A project composes modules in Nix. Codegen emits:

- SQLite schema and seed SQL
- TypeScript row/input types
- service-layer business logic
- REST API
- MCP server
- OpenAPI metadata
- generated public and admin web UI

The evaluated Nix schema is the source of truth. The generated UI is inferred from the data graph, operations, constraints, field metadata, temporal fields, and commerce/profile conventions. There is no separate hand-written UI schema.

## Model

```text
primitives
  Shared concepts such as users, profiles, temporal records, documents,
  bookings, tickets, payments, notifications, and audit.

capabilities
  Fixed implementation machinery such as auth, checkout, effects,
  migrations, generated REST/MCP/OpenAPI, and generated UI.

bundles
  Opinionated combinations of primitives and capabilities, such as
  production scheduling or materials library.

apps
  Concrete compositions for an organisation or individual.
```

Foreign keys are the primary structural signal. OpenB2C treats the schema as a graph, not a tree, so a record can appear in every relevant workspace. Profile extensions are inferred from one-to-one required unique FKs, and commerce roles are inferred from FKs, constraints, money fields, status fields, and checkout settings.

## Current Pilot

Duchy Opera is the first acceptance case:

- public performances and ticket checkout
- payment intent and payment webhook flow
- customer-owned bookings and tickets
- private production scheduling
- rehearsals, calls, requirement coverage, and materials
- generated public and private/admin surfaces

The goal is not to build a Duchy-only app. Duchy Opera is the proving lens for reusable platform modules.

## Repository

```text
schema/
├── base.nix              # Nix option model
├── lib/                  # expression and composition helpers
├── modules/              # reusable modules
├── bundles/              # cross-entity bundles
├── codegen/              # generators and tests
└── ui/                   # generated UI runtimes

examples/
└── duchyopera/           # current pilot
```

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

## Stack

- Nix for composition
- Bun and TypeScript for generated runtime code
- SQLite for storage
- REST, MCP, OpenAPI, and generated web UI outputs

## License

See [LICENSE.md](LICENSE.md).
