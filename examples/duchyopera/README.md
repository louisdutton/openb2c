# Duchy Opera

Duchy Opera is the current pilot composition for OpenB2C.

It proves that public commerce and private operations can be generated from reusable modules and bundles rather than from a Duchy-specific app screen.

## Scope

- Public performances and ticket checkout.
- Booking, ticket, transaction, and payment intent flow.
- Private production scheduling.
- Rehearsals, calls, requirement coverage, and materials.
- Artist profiles as one-to-one extensions of users.
- Generated public and admin web UI.

## Composition

`composition.nix` imports reusable modules from `schema/modules/` and bundles from `schema/bundles/`.

Commerce is enabled with:

```nix
commerce = {
  enabled = true;
  checkout = {
    currency = "GBP";
    expiryMinutes = 15;
    maxQuantity = 20;
    maxLines = 50;
  };
};
```

Catalog, order, line item, transaction, and bridge roles are inferred from the FK graph, constraints, money fields, status fields, and temporal fields.

## Commands

From the repository root:

```bash
compose examples/duchyopera/composition.nix
compose --explain examples/duchyopera/composition.nix
bun test
```

Run the generated app:

```bash
cd examples/duchyopera
bun run server
```

Fresh local databases apply reference venue data and demo fixtures by default. Set `OPENB2C_APPLY_FIXTURES=false` to start with reference data only.

## Checkout Flow

1. `POST /commerce/checkout` creates a pending order from a cart.
2. `POST /commerce/orders/{id}/payment-intent` creates or retrieves an idempotent provider payment intent.
3. `POST /commerce/payments/webhook` receives a signed provider callback and confirms line items when payment succeeds.
4. `POST /commerce/orders/expire` cancels unpaid reservations after their expiry window.

Use `PAYMENT_PROVIDER=fake` or `local` for development. Use `PAYMENT_PROVIDER=stripe` with `PAYMENT_API_KEY` and `PAYMENT_WEBHOOK_SECRET` for production-like Stripe flows.
