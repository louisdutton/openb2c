# Deployment

Deploy one generated Rust binary and one SQLite database per application. The same process serves public pages, admin pages, assets and REST. Put it behind an HTTPS reverse proxy.

## Build

```sh
devenv build outputs.duchyopera
```

The build prints the application output path. On a Nix host, retain its closure and point the service at `<output>/bin/openb2c-app`. Copying that binary alone is insufficient because it links against libraries in the Nix store; distribute its closure using Nix tooling.

For a non-Nix host, export a standalone Cargo project with `devenv shell -- cargo run -p openb2c -- examples/duchyopera/composition.nix`, then build it for the server's target platform and C library with `cargo build --release --manifest-path examples/duchyopera/generated/Cargo.toml`. Install that binary alongside operator migrations in a versioned release directory. Both builds embed the model, OpenAPI and assets.

Set `MIGRATIONS_DIR` to the release's migrations directory: the compiled-in development path is not suitable for deployment. Keep databases and backups outside release directories.

## Environment

Use a root-managed environment file such as `/etc/openb2c/duchyopera.env`:

```sh
NODE_ENV=production
HOST=127.0.0.1
PORT=3085
DB_PATH=/var/lib/openb2c/duchyopera/app.db
MIGRATIONS_DIR=/srv/openb2c/duchyopera/current/migrations
AUTH_ENABLED=true
OPENB2C_APPLY_FIXTURES=false
CORS_ORIGINS=https://duchyopera.example
REGISTRY_PRIVATE_KEY=<persistent Ed25519 seed as hex>
EMAIL_PROVIDER=resend
RESEND_API_KEY=<secret>
EMAIL_FROM=boxoffice@duchyopera.example
```

Supply integration-specific secrets from the generated `.env.example` and [configuration contract](configuration.md). Current payment adapters are local/fake; production rejects these without explicit `ALLOW_FAKE_PROVIDERS=true`, intended for controlled integration testing.

## Service

Create the state directory owned by an unprivileged `openb2c` user. A systemd unit can use:

```ini
[Unit]
Description=Duchy Opera
After=network-online.target
Wants=network-online.target

[Service]
User=openb2c
Group=openb2c
WorkingDirectory=/var/lib/openb2c/duchyopera
EnvironmentFile=/etc/openb2c/duchyopera.env
ExecStart=/srv/openb2c/duchyopera/current/openb2c-app
Restart=on-failure
TimeoutStopSec=60
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=/var/lib/openb2c/duchyopera

[Install]
WantedBy=multi-user.target
```

Proxy all routes, including pages and assets, to the process. For Caddy:

```caddyfile
duchyopera.example {
  reverse_proxy 127.0.0.1:3085
}
```

Verify `/health`, public catalog, sign-in, and an authorized admin read after deployment. Monitor structured request logs, `/ops/metrics`, and failed effects. Back up SQLite before a release; follow the [migration and restore guide](sqlite-migrations.md).

## MCP

`openb2c-app --mcp` uses newline-delimited JSON-RPC on stdin/stdout with trusted system access. Give it an explicit `DB_PATH` and `MIGRATIONS_DIR`. `openb2c-app --mcp-http` serves MCP at `/mcp` on `MCP_PORT` (default 3090); initialize a session, then send its `Mcp-Session-Id` and bearer credentials on subsequent requests. Expose it only through the intended authenticated deployment boundary.
