# OpenB2C

OpenB2C composes business capabilities in Nix and generates standalone Rust backends with SQLite, REST, and OpenAPI.

## Architecture

- The evaluated Nix schema is authoritative. Foreign keys define the relationship graph.
- Keep the pilot backend-only and use REST as its single transport. Declare business behavior explicitly; foreign keys describe structure, not permission or lifecycle semantics.
- Protect lifecycle fields through explicit operations. Disable generic operations that would bypass the workflow.
- Keep capabilities independently selectable. Consumers declare versioned dependencies and role bindings; providers must not depend on their consumers.
- Default integrations on when both capabilities are enabled. Keep opt-outs on consumers (`orders.payments.enable`, `orders.entitlements.enable`, `subscriptions.entitlements.enable`), not separate top-level integration flags.
- Use the fake payment adapter only. No live payment processing is implemented or authorized for the pilot.
- Implement reusable capabilities in the platform; keep application-specific composition in examples.
- Change source models or generators and rebuild. Do not hand-edit generated code, including CLI exports under `generated/`.

## Source map

- `nix/model/capabilities/`: selectable B2C modules and explicit consumer integrations.
- `nix/model/`: Nix options, application composition, expression helpers and generic modules.
- `crates/openb2c-schema/`: capability contracts, validation, graph inspection, SQL and OpenAPI.
- `crates/openb2c/`: generator CLI and Rust model generation.
- `crates/openb2c-runtime/src/b2c/`: independent capability services, integration ports and REST adapter.
- `crates/openb2c-runtime/`: shared auth, SQLite, audit and effects.
- `devenv.nix` and `devenv.yaml`: root development configuration.
- `nix/build/`: packaging and source-generation helpers, plus the generated application's dependency lock.
- `examples/publisher.nix`: standalone digital publisher module; selects modules through enable options registered by the base evaluator.

## Schema conventions

- Put single-field constraints beside the field: `constraints.enum`, `constraints.length`, `constraints.range`, and `constraints.pattern`. Put multi-field constraints under `constraints.<entity>.unique` or `constraints.<entity>.check`.
- Declare structure with `references = "target(id)"`. Field `relationship` metadata may describe that FK but must not create another relationship model.
- Use `config.refs`, `config.values`, `config.entityRefs`, and `config.effectRefs` for field, enum, entity, and effect references.
- Keep operation semantics separate from presentation. Typed assignments use exactly one source: `value`, `now`, `expression`, `input`, or `clear`.
- Read option definitions in `nix/model/base.nix`, capability modules and their tests for the detailed model contract.

## Development conventions

- Use the Linux-only Devenv environment and its built-in Rust language support.
- Use processes for long-running services, tasks for actions, and outputs for generated artifacts. Services should consume their outputs directly.
- Declare dependencies on consumers: use `after` for task/process ordering and derivation references for build dependencies.
- Build Rust packages with `languages.rust.import`. Use built-in git hooks for formatting and linting rather than duplicating checks in `enterTest`.
- Keep Devenv configuration, inputs, outputs and processes at the repository root. Each example is a standalone named module under `examples/` with no imports into the platform's directory tree. Select capabilities with `<name>.enable`; dependencies remain explicit on consumers. Use `config.git.root` for repository paths.
- Put application `name`, `description` and default `currency` first at the top level. Use named `demo.products` with nested prices for sample catalogs; keep IDs, foreign keys and bootstrap details out of examples.
- Consult the official Devenv documentation before introducing custom machinery for a supported feature.

## Verification

- Use `devenv shell` for development and `devenv test` for hooks and workspace tests. See [README.md](README.md) for application build/run commands.
- Run checks appropriate to the change. Documentation-only edits do not require application tests.
- For generator or runtime changes, build an affected generation output and application binary. For workflow changes, exercise the affected REST flow, including authorization and rejected state changes.
- For capability changes, test partial compositions and dependency failures as well as the full pilot. Verify replay, rollback and expiry behavior for financial/access state changes.
- Use isolated test databases; preserve existing application data and operator-owned migrations.
- Treat migrations as complete only after behavior is verified, consumers are updated, and superseded implementations are removed. Report any remaining gaps explicitly.
