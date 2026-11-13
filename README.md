# OpenB2C

OpenB2C composes reusable business capabilities in Nix and generates standalone Rust applications: SQLite storage, typed models, REST, MCP, OpenAPI, and public and admin web pages.

The evaluated Nix schema is authoritative. Foreign keys define the data graph; operations, constraints, field metadata, and temporal fields determine workflows and presentation. There is no parallel UI schema.

## Run

Install [Devenv](https://devenv.sh/getting-started/) on Linux, then:

```sh
devenv shell
devenv build outputs.duchyopera-generated
cat "$(devenv build outputs.duchyopera-generated | jq -r '."outputs.duchyopera-generated"')/inference.md"
devenv up duchyopera
```

`devenv up duchyopera` consumes the application output directly. Nix builds the generator, generated sources and application in dependency order and caches each result. There is no generation task that writes into the checkout. Generated sources and inference reports are available through `outputs.duchyopera-generated`; `outputs.duchyopera` contains the runnable binary, and `outputs.generator` contains the CLI.

Duchy Opera owns its environment in `examples/duchyopera/devenv.nix`. These commands also work after `cd examples/duchyopera`. The root environment imports the example, so it can run the same process and build the same outputs.

Open `http://127.0.0.1:3085/` for the public site, `/account` for customer records, and `/admin` for staff. Email verification uses captured development messages. Staff can sign in with an API key; see [authentication](docs/auth-and-authorization.md). To explore locally with system access, run `AUTH_ENABLED=false devenv up duchyopera`.

The generated application serves HTML, CSS, assets, and APIs from one Rust process. Forms use ordinary HTTP and CSRF protection. No JavaScript build or runtime is required.

```sh
devenv build outputs.duchyopera
devenv test
```

Normal HTTP listens on port 3085; MCP HTTP uses 3090 and requires initialization and authenticated subsequent requests. Stdio MCP is trusted local access. Both transports use the same services and database settings.

## Structure

```text
schema/                        Nix option model, helpers, modules and bundles
crates/openb2c-schema/          Schema, validation, graph inference, SQL and OpenAPI
crates/openb2c/                 Native generator CLI and Rust model generation
crates/openb2c-runtime/         SQLite services, auth, commerce, effects, HTTP/MCP and UI
devenv/shared/                 Shared Rust tooling, hooks, inputs and workspace tests
nix/                           Generator packaging and reusable source generation
examples/duchyopera/            Pilot composition, assets and its own Devenv environment
docs/                          Schema contracts and operating guides
```

Devenv generation outputs are immutable directories in the Nix store. They contain a Cargo manifest, locked dependencies, support crates, SQL, assets and inference reports. Change the Nix model or source crates and rebuild; do not hand-edit generated files. The CLI still supports writing a standalone project to a composition's `generated/` directory for export or development. Operator migrations and databases belong outside immutable outputs.

## Acceptance case

Duchy Opera exercises public performance booking, ticket checkout, payments, customer accounts, private production scheduling, rehearsals, calls, requirement coverage, materials, and artist profiles. Commerce roles and workspaces come from the graph so the same machinery applies to other compositions.

Payment adapters currently support local/fake settlement, including signed webhook, expiry and refund flows. Production payment providers require their own adapter; setting a provider name does not implement it.

## Development and deployment

Devenv's [built-in Rust support](https://devenv.sh/languages/rust/) supplies Cargo, rustc, rustfmt, Clippy and rust-analyzer from the nixpkgs input pinned in `devenv.lock`. `devenv update` updates environment inputs deliberately; Cargo dependencies are pinned in `Cargo.lock`. `.envrc` uses `use devenv`. Nix compositions resolve `<nixpkgs/lib>` from the same pinned input.

Both binary outputs use `languages.rust.import`, Devenv's built-in crate2nix packaging function. `nix/generator.nix` packages the tool and `nix/generate.nix` generates sources for an evaluated schema. Duchy Opera declares its own model, outputs and process in its Devenv module. `nix/generated-app.lock` pins the common generated application manifest; refresh it with Cargo when the generated dependency contract changes. Lifecycle dependencies are declared on consumers: the server references its application output, and `devenv:enterTest.after` selects the Rust test task.

The environment follows Devenv's [monorepo pattern](https://devenv.sh/guides/monorepo/): YAML imports beginning with `/` resolve from the Git repository root. The root imports `/devenv/shared` and `/examples/duchyopera`; the example imports `/devenv/shared`. Shared inputs are declared once in `devenv/shared/devenv.yaml`. The active environment uses its own lockfile: the root lock for the combined environment, and `examples/duchyopera/devenv.lock` when working directly in the example. Both start from the same pinned inputs. Repository paths use `config.git.root`, so working directories stay consistent from either entry point.

The pinned `git-hooks` input enables the built-in Rustfmt and Clippy hooks, with warnings denied across all workspace targets. Devenv installs these on shell entry and runs them during `devenv test`. The `openb2c:test` task also runs service and contract tests, database upgrades, delivery retries, generated model compilation, and Chromium customer/admin journeys. Tests launch their own isolated servers; the development server is excluded from the test environment. See [migration details](docs/rust-migration.md), [configuration](docs/configuration.md), [deployment](docs/deployment.md), and [SQLite migrations](docs/sqlite-migrations.md).

Licensed under [GPL-3.0](LICENSE.md).
