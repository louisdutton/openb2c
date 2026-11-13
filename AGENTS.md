# OpenB2C

OpenB2C composes business capabilities in Nix and generates standalone Rust applications with SQLite, REST, MCP, OpenAPI, and public/admin web surfaces.

## Architecture

- The evaluated Nix schema is authoritative. Foreign keys define the relationship graph.
- Infer UI from the graph, operations, constraints, metadata, and temporal fields. Add a separate UI schema only when inference cannot express the workflow.
- Implement reusable capabilities in the platform; keep application-specific composition in examples.
- Change source models or generators and rebuild. Do not hand-edit generated code, including CLI exports under `generated/`.

## Source map

- `schema/`: Nix options, expression helpers, reusable modules and bundles.
- `crates/openb2c-schema/`: validation, graph inference, SQL and OpenAPI.
- `crates/openb2c/`: generator CLI and Rust model generation.
- `crates/openb2c-runtime/`: services, auth, commerce, effects, HTTP/MCP and HTML UI.
- `devenv/shared/` and `nix/`: shared development configuration and packaging/generation helpers.
- `examples/`: application compositions, assets and their own Devenv environments.

## Schema conventions

- Put single-field constraints beside the field: `constraints.enum`, `constraints.length`, `constraints.range`, and `constraints.pattern`. Put multi-field constraints under `constraints.<entity>.unique` or `constraints.<entity>.check`.
- Declare structure with `references = "target(id)"`. Field `relationship` metadata may describe that FK but must not create another relationship model.
- Use `config.refs`, `config.values`, `config.entityRefs`, and `config.effectRefs` for field, enum, entity, and effect references.
- Keep operation semantics separate from presentation. Typed assignments use exactly one source: `value`, `now`, `expression`, `input`, or `clear`.
- See [workflow metadata](docs/workflow-metadata.md), [field metadata](docs/field-metadata.md), and [validation metadata](docs/validation-metadata.md) for details and examples.

## Development conventions

- Use the Linux-only Devenv environment and its built-in Rust language support.
- Use processes for long-running services, tasks for actions, and outputs for generated artifacts. Services should consume their outputs directly.
- Declare dependencies on consumers: use `after` for task/process ordering and derivation references for build dependencies.
- Build Rust packages with `languages.rust.import`. Use built-in git hooks for formatting and linting rather than duplicating checks in `enterTest`.
- Each example owns its application outputs and processes. Import shared configuration and examples using Devenv's repository-root YAML imports; use `config.git.root` for repository paths.
- Consult the official Devenv documentation before introducing custom machinery for a supported feature.

## Verification

- Use `devenv shell` for development and `devenv test` for hooks and workspace tests. See [README.md](README.md) for application build/run commands.
- Run checks appropriate to the change. Documentation-only edits do not require application tests.
- For generator or runtime changes, build an affected generation output and application binary. For workflow changes, exercise the affected customer/admin flow.
- Use isolated test databases; preserve existing application data and operator-owned migrations.
- Treat migrations as complete only after behavior is verified, consumers are updated, and superseded implementations are removed. Report any remaining gaps explicitly.
