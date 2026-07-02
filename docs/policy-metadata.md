# Policy Metadata

Runtime authorization is enforced by scopes, public flags, and record relationships. Policy metadata only describes intended audience and risk for generated docs and clients.

```nix
operations.ticket.confirm = {
  policy = {
    audiences = [ "customer" "service" ];
    risk = "medium";
  };
  guard = E.eq (E.f F.status) (E.lit V.status.reserved);
};
```

Supported audiences:

- `anonymous`
- `customer`
- `staff`
- `service`
- `system`

When `audiences` is omitted, codegen derives it:

- public operations: anonymous/customer/staff/service
- relationship-scoped operations: customer
- protected global operations: staff/service

Presentation concerns such as label, grouping, display priority, and confirmation live under `operation.presentation`, not `policy`.

Generated OpenAPI includes `x-openb2c-auth` and `x-openb2c-policy`. Enforcement still follows [Authentication And Authorization](./auth-and-authorization.md).
