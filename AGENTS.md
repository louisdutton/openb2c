# OpenB2C

The project composes business software in Nix and compiles bespoke standalone Rust services with SQLite, REST, and OpenAPI. OpenB2C is a historical name, not a restriction to consumer-facing software.

## Architecture

- Compile the composition; share the implementation. Nix is the business composition language: it selects capabilities and stitches algorithms together through explicit workflows, transitions, policies, dependencies and bindings. Rust implements defined algorithms and protocol behavior, including persistence, cryptography, accounting calculations and HTTP.
- Resolve all application structure and executable policy during Nix evaluation and code generation. Generate concrete Rust types, calls, routes, event wiring, authorization predicates and database mappings for the selected composition. Use the generated types in execution; do not generate unused types beside an interpreter.
- Keep reusable Rust implementations independent of application composition. A business workflow or consumer integration belongs in Nix; do not reproduce it in handwritten Rust registries, capability-name switches or schema-driven event dispatch. Generate the wiring to the Rust implementations.
- The evaluated model is a build-time intermediate representation. Schema metadata may be retained for migrations, inspection and API documentation; it must not become an application rules interpreter. Reject unsupported constructs during evaluation or generation rather than silently falling back to runtime interpretation.
- Compile-time knowledge concerns structure and rules, not live facts. Customer records, catalog data, staff memberships, credentials, clock readings and external outcomes remain runtime inputs, validated by compiled code. Deployment settings may supply secrets, addresses, data paths and resource limits; they must not select capabilities or replace compiled business policy.
- Generate only the selected application and its required implementation dependencies. Shared Rust libraries are expected, but shipping a universal application engine and a JSON model is not the target architecture. Optimize away model discovery and repeated schema lookups from request execution rather than assuming Rust compilation alone removes them.
- The workspace `reference` feature is a regression-test harness, not a deployable execution model. Exported services must disable and reject it. Add business wiring in Nix and verify it in generated-application tests; do not extend the reference consumers with new business orchestration. Preserve data, authorization, replay, rollback and upgrade guarantees throughout.
- The evaluated Nix schema is authoritative. Foreign keys define the relationship graph.
- Keep the pilot backend-only and use REST as its single transport. Declare business behavior explicitly; foreign keys describe structure, not permission or lifecycle semantics.
- Protect lifecycle fields through explicit operations. Disable generic operations that would bypass the workflow.
- Keep capabilities independently selectable. Consumers declare versioned dependencies and role bindings; providers must not depend on their consumers.
- Default integrations on when both capabilities are enabled. Keep opt-outs on consumers (`orders.payments.enable`, `orders.entitlements.enable`, `subscriptions.entitlements.enable`), not separate top-level integration flags.
- Payments are customer-initiated UK bank transfers. Rust may read the business's own bank feed and verify signed notifications, but must never initiate payments, refunds or standing orders. Keep the publisher's default feed simulated; use isolated simulated transactions for verification. Never contact a live bank account during tests.
- Keep requests, bank facts and allocations separate. Persist confirmed bank facts before business reactions; rejected fulfilment must retain the receipt for review. Match automatically only by compiled Nix policy. Require operator authority for manual allocation, retain ambiguous funds, and prevent duplicate allocation.
- Scope financial capabilities to the UK. Preserve source amounts and tax treatment; journals balance in GBP, corrections use reversals, and closed periods reject backdated postings. VAT drafts require explicit classification and review; do not imply HMRC filing or statutory compliance.
- Credits and cash refunds are separate operations. Accept provider facts durably when a closed period defers accounting; preserve source dates and require an open posting date.
- Platform accounts and staff memberships are independent of customer records. Passwords are built in; public signup creates customer membership only and never grants staff authority.
- Populated capability transitions require review bound to the model, data and operator SQL. Preserve historical tables and role data, block unresolved retirement, and verify backup/restore and credential recovery before expanding capabilities.
- Implement reusable capability composition in platform Nix modules and reusable algorithms in Rust; keep application-specific composition in examples. Existing `b2c`, `backoffice` and `runtime` directory names describe current files, not required architectural boundaries.
- Change source models or generators and rebuild. Do not hand-edit generated code, including CLI exports under `generated/`.

## Source map

- `nix/model/capabilities/`: selectable business modules and explicit consumer integrations.
- `nix/model/lib/{workflow,reactions}.nix`: declarative event and background compositions compiled into application code.
- `nix/model/`: Nix options, application composition, expression helpers and generic modules.
- `crates/openb2c-schema/`: capability contracts, validation, graph inspection, SQL and OpenAPI.
- `crates/openb2c/`: generator CLI, selected implementation packaging and compiled application generation. `src/compile.rs` lowers typed event reactions and REST wiring; `src/compile/{records,api,typed}.rs` emit concrete storage, operations and native expressions. `src/compile/operations.rs` supplies transport authorization, metadata and listings.
- `crates/openb2c-runtime/src/b2c/`: independent capability services, integration ports and REST adapter.
- `crates/openb2c-runtime/src/backoffice/`: internal accounting, billing, purchasing, document storage, reconciliation and UK VAT reports.
- `crates/openb2c-runtime/`: shared auth, SQLite, audit and effects.
- `crates/openb2c-runtime/src/{accounts,operations,transitions}.rs`: platform identity, local recovery/backup commands and reviewed upgrades.
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
- Workspace reference tests alone do not prove Nix wiring. Build and execute the generated implementation with reference execution disabled; verify an application-owned Nix override and fail generation for invalid composition without changing existing output.
- For capability changes, test partial compositions and dependency failures as well as the full pilot. Verify replay, rollback and expiry behavior for financial/access state changes.
- Use isolated test databases; preserve existing application data and operator-owned migrations.
- Treat migrations as complete only after behavior is verified, consumers are updated, and superseded implementations are removed. Report any remaining gaps explicitly.
