# Rust migration

The generator and generated applications use Rust. The TypeScript generator, browser component runtime, Bun scripts and package manifests have been removed. Nix remains the composition language and evaluated schema contract.

## Architecture

The workspace separates schema interpretation (`openb2c-schema`), generation (`openb2c`) and execution (`openb2c-runtime`). Generated applications copy the schema and runtime crates into `support/`, emit typed serde row/input/patch models, and embed schema, OpenAPI, CSS and local branding assets in the binary. There are no repository-relative runtime dependencies.

Axum handles HTTP, rusqlite owns SQLite transactions, and native MCP exposes the same service operations. Public and admin pages are rendered in Rust; navigation and forms use HTTP rather than browser components. The public site and API now share one origin and port. Existing hash-based browser routes should be replaced with `/`, `/catalog`, `/account` and `/admin` links.

Services enforce scopes, record relationships, guards, transitions, assignments, cascades, validation, derived fields, optimistic concurrency, redaction, audit and transactional effects. Identity supports OTP, bcrypt sessions and keys, Ed25519 certificates, rotation/revocation and replay protection. Commerce infers graph roles and implements capacity-safe checkout, expiry, payment sessions and idempotent settlement/refund handling. A separate SQLite connection runs expiry and effect delivery with retries and dead letters.

## Compatibility

SQLite columns, foreign keys, indexes, seeds and migration checksums are checked against a captured legacy contract. An upgrade test opens a legacy database and verifies preservation of records and migration history. Existing bcrypt credentials remain verifiable. Numbered migrations remain operator-owned and survive generation.

Rust callers use `AuthContext { user_id, scopes }` and native service methods. Generated `Input` omits absent optional fields so database defaults apply. `Patch` distinguishes an absent field from explicit null. Secret and server-managed fields are excluded from public models or write inputs.

The JSON API retains collection/item/custom operation paths, raw response objects, error codes, ETags and version headers. MCP stdio is trusted local system access; remote MCP HTTP requires an initialized session and authentication. The default database name is the organization slug plus `.sqlite` in the working directory for every transport; set `DB_PATH` explicitly when upgrading.

Payment support remains local/fake, matching the previously implemented provider boundary. Production payment adapters are separate future work, not part of the language migration.

## Environment

`devenv/shared/devenv.nix` uses `languages.rust.enable = true` and the nixpkgs channel. Cargo, rustc, rustfmt, Clippy and rust-analyzer share the pinned environment. No rustup or separate overlay is required. The root and Duchy Opera each have a `devenv.yaml`, `devenv.lock` and `.envrc`; shared input declarations live in `devenv/shared/devenv.yaml`.

Following Devenv's [monorepo guide](https://devenv.sh/guides/monorepo/), the root imports `/devenv/shared` and `/examples/duchyopera`, and the example imports `/devenv/shared`. Duchy Opera owns its application outputs and process. Both entry points expose the same output names and use `config.git.root` for repository paths. Each active environment resolves its own lockfile; imported lockfiles are not merged. The shared workspace test task runs from the Git root in either environment.

The development environment requires Linux and includes Chromium and ChromeDriver. Generation is a Nix derivation exposed as `outputs.duchyopera-generated`, with sources, a lockfile and inference reports. `outputs.generator` and `outputs.duchyopera` use the built-in `languages.rust.import` function. The managed server references the application output directly, so the build dependency belongs to the consumer and requires no ordering task.

The virtual development workspace is adapted to a standalone generator packaging view for crate2nix's root-package API. Cargo resolves the packaging graph offline from the pinned lockfile and vendored sources. The original manifests remain the source for emitted support crates. `nix/generated-app.lock` pins the shared generated application dependency contract; refresh it with Cargo when that contract changes.

Rustfmt and strict workspace Clippy use the built-in git hooks. Application tests remain a task selected by `tasks."devenv:enterTest".after = [ "openb2c:test" ]`. There is no custom `enterTest` shell block or generation task.

This follows the official [Rust module](https://devenv.sh/languages/rust/), [outputs](https://devenv.sh/outputs/), [input locking](https://devenv.sh/inputs/), [tasks](https://devenv.sh/tasks/), [processes](https://devenv.sh/processes/) and [git hooks](https://devenv.sh/git-hooks/) documentation.

## Verify

```sh
devenv test
devenv build outputs.duchyopera-generated
devenv build outputs.generator outputs.duchyopera
```

Tests cover native authorization, operations, constraints, identity, commerce, concurrent reservations, database upgrades, HTTP/OpenAPI/MCP contracts, signed delivery retries, generated model compilation, and real Chromium customer/admin flows. The captured legacy SQL fixture is test data; tests do not invoke the removed implementation.
