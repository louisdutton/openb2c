# Seed Data

Seed data declares deterministic rows in the Nix composition.

- `seed.reference.<table>` is stable app-controlled data and is applied idempotently at startup.
- `seed.fixtures.<table>` is demo data and is applied only when fixtures are enabled.

Every seed row must include a primary key or unique field.

```nix
seed.reference.venue = [
  {
    id = 1;
    name = "Hall for Cornwall";
    city = "Truro";
    postcode = "TR1 2LL";
    capacity = 900;
  }
];

seed.fixtures.performance = [
  {
    id = 1;
    production_id = 1;
    venue_id = 1;
    starts_at = "2026-06-27T19:30:00Z";
    ends_at = "2026-06-27T22:00:00Z";
    price_pence = 2500;
    status = "scheduled";
  }
];
```

Generated outputs include `seed.sql` and `fixtures.sql`.

Reference data is applied automatically. Fixtures are applied when `OPENB2C_APPLY_FIXTURES=true`, or when `seed.applyFixturesByDefault = true` in non-production. Set `OPENB2C_APPLY_FIXTURES=false` to disable a non-production default.
