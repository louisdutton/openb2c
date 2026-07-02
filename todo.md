# Current Agenda

OpenB2C is past the initial schema-language prototype. The next priority is proving the platform through a feature-complete Duchy Opera pilot while keeping every improvement reusable.

## Implemented Foundation

- Nix composition for reusable modules and bundles.
- SQLite, TypeScript services, REST, MCP, OpenAPI, public UI, and admin UI generation.
- FK-first relationship inference and graph-derived workspaces.
- Field metadata, typed constraints, profile-extension inference, and commerce-role inference.
- Typed operation assignments, transitions, presentation metadata, policy metadata, audit metadata, cascades, and typed effects.
- Auth context, scopes, relationship authorization, API keys, identity sessions, and certificate request auth.
- Seed data, migrations, startup diagnostics, structured logs, and generated `.env.example`.
- Local/fake providers plus provider-neutral payment-session, email, and webhook integration contracts.
- Browser and integration tests for generated public/private flows.

## Next: Duchy Opera Pilot

- Add production Stripe/Adyen adapters after the local provider-neutral flow is stable.
- Improve admin production workflows for performances, rehearsals, calls, coverage, and materials.
- Validate generated UI on realistic data volume and mobile layouts.
- Document the operational runbook for deploying and backing up the pilot.

## Framework Cleanup To Do When Needed

- Split remaining large generator files where maintenance pressure is real.
- Add more generator snapshot tests.
- Add CI for tests and example generation checks.
- Define stable module authoring conventions.
- Add dependency/conflict checks between modules and bundles.
- Add a blank project template.

## Deferred

- Full audit-log product surface.
- Rich role/permission management UI.
- Calendar/provider sync.
- Real document storage and upload/download.
- SSR or separately deployed frontend architecture.
- Broader payment/refund/reconciliation support beyond the first checkout flow.
