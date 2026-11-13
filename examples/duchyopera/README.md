# Duchy Opera

Duchy Opera is the current pilot composition for OpenB2C.

It proves that public commerce and private operations can be generated from reusable modules and bundles rather than from a Duchy-specific app screen.

## Scope

- Public performances and ticket checkout.
- Booking, ticket, transaction, and payment session flow.
- Private production scheduling.
- Rehearsals, calls, requirement coverage, and materials.
- Artist profiles as one-to-one extensions of users.
- Generated public and admin web UI.

## Composition

`model.nix` imports reusable modules from `schema/modules/` and bundles from `schema/bundles/`, accepting the caller's pinned Nix library. `composition.nix` is the CLI entry point for the same model.

Commerce is enabled with:

```nix
commerce = {
  enabled = true;
  checkout = {
    currency = "GBP";
    expiryMinutes = 15;
    paymentDeadlineMinutes = 30;
    allowAsyncPayments = false;
    maxQuantity = 20;
    maxLines = 50;
  };
};
```

Catalog, order, line item, transaction, and bridge roles are inferred from the FK graph, constraints, money fields, status fields, and temporal fields.

## Commands

Enter this example's own environment:

```bash
cd examples/duchyopera
devenv shell
```

Its `devenv.yaml` imports `/devenv/shared` using Devenv's repository-root monorepo imports. Shared Rust tooling, hooks, the generator and workspace acceptance tests are inherited. This example owns its generated output, application output and server process. `config.git.root` anchors repository paths regardless of the active environment directory.

From here, or from the repository root which imports this environment:

```bash
devenv build outputs.duchyopera-generated
cat "$(devenv build outputs.duchyopera-generated | jq -r '."outputs.duchyopera-generated"')/inference.md"
devenv test
```

Run the generated public/admin UI against the generated API:

```bash
devenv up duchyopera
```

Public pages, admin pages and the API share `http://127.0.0.1:3085`. The process consumes `outputs.duchyopera`, which depends on the immutable generated source output. Nix builds both Rust binaries with `languages.rust.import`. Use `AUTH_ENABLED=false devenv up duchyopera` for local system access, or sign in through email/API key. Fresh local app databases apply reference venue data and demo fixtures by default. Set `OPENB2C_APPLY_FIXTURES=false` to start with reference data only.

## Checkout Flow

1. `POST /commerce/checkout` creates a pending order from a cart.
2. `POST /commerce/orders/{id}/payment-session` creates or retrieves an idempotent provider payment session, starts the bounded payment lock, and returns a provider-neutral `next_action`.
3. The local/fake provider `next_action` opens a simulator that sends signed provider-style events to `POST /commerce/payments/webhook`.
4. `POST /commerce/orders/expire` cancels unpaid reservations after their checkout or payment deadline window.

Use `PAYMENT_PROVIDER=fake` or `local` for development. Production provider adapters such as Stripe or Adyen are added after this provider-neutral contract.
