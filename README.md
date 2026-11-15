# OpenB2C

OpenB2C helps businesses own and adapt their software. Compose business capabilities in Nix and generate a standalone Rust backend with SQLite, REST and OpenAPI. The business keeps its source, data and deployment.

The [publisher example](examples/publisher.nix) combines customers, a catalog, one-off purchases, payments, subscriptions and access rights. Payments are simulated locally; no real payment adapter is included.

## Run

From the repository root, on Linux with [Devenv](https://devenv.sh/getting-started/):

```sh
devenv shell
devenv build outputs.publisher-generated outputs.publisher
devenv up publisher
```

The server listens on `http://127.0.0.1:3085`. `/health` reports status, `/b2c/catalog` shows products, and `/openapi.json` describes the enabled endpoints, request formats and required scopes.

The example uses `publisher.sqlite` in the repository root. Demo products load by default in development and are disabled by default in production. Set `OPENB2C_APPLY_FIXTURES=false` to skip demo loading; existing rows are not deleted.

The example enables `auth.password.enable`. Sign up directly over HTTP:

```sh
curl -fsS http://127.0.0.1:3085/auth/signup \
  -H 'Content-Type: application/json' \
  -d '{"email":"customer@example.test","password":"violet-river-sunrise-47"}'
```

Send the returned `access_token` as `Authorization: Bearer <token>`. Sessions expire after one hour. `POST /auth/login` accepts the same email/password fields; `POST /auth/logout` revokes the current session. `POST /auth/password` accepts `current_password` and a new `password`, revokes previous sessions and returns a new token. Passwords must contain 15–128 characters. Customer permissions are assigned by the service.

Use HTTPS outside local development. Email ownership verification and forgotten-password recovery are not implemented; an email address is currently an account identifier.

For operators and automation, provision a scoped API key from another Devenv shell:

```sh
app=$(devenv build outputs.publisher | jq -er '."outputs.publisher"')
"$app/bin/openb2c-app" --create-api-key operator@example.test 'payments.simulate,subscriptions.renew'
```

Send the returned key as `Authorization: Bearer <key>`. Customer signup never grants these operator permissions.

## Change the application

Edit `examples/publisher.nix` and rebuild. It is an ordinary Nix module: application details come first, capabilities use top-level `<name>.enable` options, and named demo products inherit the global currency. Consumers declare their dependencies; unrelated capabilities remain optional.

Enabled capabilities work together by default: orders use payments, and orders/subscriptions grant access when entitlements are enabled. Opt out on the consumer with `orders.payments.enable = false`, `orders.entitlements.enable = false`, or `subscriptions.entitlements.enable = false`.

The evaluated model is authoritative. Change source modules or generators rather than generated files. Business lifecycles are enforced by capability services; generic table writes cannot bypass payment, order, subscription or access rules.

Build outputs are:

- `outputs.generator`: the generator CLI.
- `outputs.publisher-generated`: standalone Rust sources, SQL, OpenAPI and dependency lock.
- `outputs.publisher`: the application binary used by the publisher process.

To export a Cargo project:

```sh
devenv shell -- cargo run -p openb2c -- --out generated examples/publisher.nix
```

Existing databases need reviewed migrations when their schema changes. Keep application data and operator-owned migrations outside immutable build outputs.

## Development

```sh
devenv test
```

The root Devenv environment provides Rust, formatting and Clippy hooks, workspace tests, build outputs and the publisher process. Rust packages build through `languages.rust.import`.

`nix/model/` contains the modelling system; `nix/build/` contains packaging and generation helpers. `crates/` contains the schema compiler, generator and runtime. Tests and option definitions describe the detailed behavior.

The pilot is backend-only. Live billing, taxes, invoices, shipping, inventory and proration are not implemented.

Licensed under [GPL-3.0](LICENSE.md).
