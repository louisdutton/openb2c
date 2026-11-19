# OpenB2C

OpenB2C helps businesses own and adapt their software. Compose business capabilities in Nix and generate a standalone Rust backend with SQLite, REST and OpenAPI. The business keeps its source, data and deployment.

The [publisher example](examples/publisher.nix) combines customers, a catalog, one-off purchases, bank transfers, subscriptions, access rights and protected downloads with internal accounting, billing, documents, purchasing, banking and UK VAT reports. Its bank feed is simulated by default. Optional Open Banking and Starling adapters read the business's own account; the service never initiates payments.

## Compilation model

The intended architecture is **Nix composition → validated model → concrete Rust → standalone service**. OpenB2C is a historical name; the model covers business software, including internal operations.

Nix expresses the business composition: selected capabilities, workflow steps, state transitions, authorization rules, event connections and dependency bindings. Rust supplies defined algorithms and protocol behavior, such as password hashing, database transactions, HTTP handling and accounting calculations. Generation stitches these implementations into a concrete application. Reusable business compositions belong in Nix modules; reusable algorithms belong in Rust libraries.

Routes, types, queries, policies and workflow wiring should be resolved before the service runs. Generated Rust should use concrete types and direct calls, include only the selected composition and its dependencies, and let the Rust compiler check the assembled program. Unsupported composition must fail evaluation or generation. The evaluated schema can support migrations, inspection and OpenAPI without being interpreted to execute application rules.

Live facts still arrive at runtime: customer and catalog records, credentials, staff permissions, dates and external results. Compiled rules validate those facts. Deployment configuration supplies secrets, addresses, paths and resource limits, rather than changing the application's business structure or executable policy.

Generated applications compile Nix event reactions and background workflows into Rust functions with typed records, explicit algorithm calls and fixed relationship queries. REST wiring, capability bindings, authorization predicates, guards, assignments, constraints, derived fields and cascades are generated for the selected model. Unselected capability implementation files are omitted. Unsupported operations, missing fields and incompatible workflow arguments fail generation before existing output is changed.

The generated service uses concrete database records, enum states and typed writes throughout purchases, payments, subscriptions, access, files, accounting, billing and purchasing. Custom application operations use the same records and mappings. Omitted fields, explicit nulls and values remain distinct; database defaults apply to omitted fields. Guards, constraints, derived fields and cascades execute as native Rust expressions. There is no separate set of unused generated model types.

JSON remains at serialization boundaries: REST adapters, the build-time model, audit and replay snapshots, durable command payloads and external delivery bodies. Stored billing commands deserialize into an enum before execution; business algorithms do not exchange JSON records. Operational inspection endpoints can serialize database output directly.

The shared Rust support crate retains its historical `openb2c-runtime` name. It supplies algorithms and technical mechanisms; the generated service uses compiled composition instead of its reference interpreter. The `reference` feature and `reference/` capability implementations exist for the workspace's in-process regression tests and are forbidden in exported applications. Generated-application tests exercise Nix overrides, custom record operations, typed storage semantics, transaction rollback, model binding and partial compositions.

The embedded model is retained for inspection and reviewed migrations. A compiled application rejects a different model rather than adapting its behavior at startup. Changing business wiring requires regeneration and rebuilding; populated databases also require the reviewed upgrade workflow below. A compact database revision check stops stale requests and background work after an upgrade.

## Run

From the repository root, on Linux with [Devenv](https://devenv.sh/getting-started/):

```sh
devenv shell
devenv build outputs.publisher-generated outputs.publisher
devenv up publisher
```

The server listens on `http://127.0.0.1:3085`. `/health` reports status, `/b2c/catalog` shows products, and `/openapi.json` describes the enabled endpoints, request formats and required scopes.

The example uses `publisher.sqlite` in the repository root. Demo products load by default in development and are disabled by default in production. Set `OPENB2C_APPLY_FIXTURES=false` to skip demo loading; existing rows are not deleted.

Platform accounts, passwords and revocable bearer sessions are built in. `customers.enable` adds public signup and a separate customer membership; staff accounts need no customer capability. Sign up directly over HTTP:

```sh
curl -fsS http://127.0.0.1:3085/auth/signup \
  -H 'Content-Type: application/json' \
  -d '{"email":"customer@example.test","password":"violet-river-sunrise-47"}'
```

Send the returned `access_token` as `Authorization: Bearer <token>`. Sessions expire after one hour. `POST /auth/login` accepts the same email/password fields; `POST /auth/logout` revokes the current session. `POST /auth/password` accepts `current_password` and a new `password`, revokes previous sessions and returns a new token. Passwords must contain 15–128 characters. Customer permissions are assigned by the service.

Use HTTPS outside local development. Email ownership verification and self-service email recovery are not implemented; an email address is currently an account identifier. Local owners can recover credentials as described below.

For operators and automation, provision a scoped API key from another Devenv shell:

```sh
app=$(devenv build outputs.publisher | jq -er '."outputs.publisher"')
"$app/bin/openb2c-app" --create-api-key operator@example.test 'payments.import,payments.reconcile,subscriptions.renew'
```

Send the returned key as `Authorization: Bearer <key>`. Customer signup never grants these operator permissions.

Staff use the same `/auth/login` endpoint with explicitly granted permissions. Local owners can provision staff without creating customer records; passwords come from stdin rather than command arguments:

```sh
"$app/bin/openb2c-app" --create-staff owner@example.test '*' --password-stdin < /run/secrets/owner-password
```

Use narrower scopes for ordinary staff. `--disable-staff EMAIL` revokes staff access, sessions and API keys. Customer signup never grants staff membership or internal permissions.

## Change the application

Edit `examples/publisher.nix` and rebuild. It is an ordinary Nix module: application details come first, capabilities use top-level `<name>.enable` options, and named demo products inherit the global currency. Consumers declare their dependencies; unrelated capabilities remain optional.

Enabled capabilities work together by default: orders use payments, and orders/subscriptions grant access when entitlements are enabled. Opt out on the consumer with `orders.payments.enable = false`, `orders.entitlements.enable = false`, or `subscriptions.entitlements.enable = false`.

The evaluated model is authoritative. Change source modules or generators rather than generated files. Business lifecycles are enforced by capability services; generic table writes cannot bypass payment, order, subscription or access rules.

Own application behaviour in ordinary Nix source modules, importing business-owned files through `imports`. The evaluator supplies `config`, `lib` and `expr`; use typed references, operation guards and assignments to define application-owned workflows. The [editorial workflow regression fixture](crates/openb2c/tests/fixtures/editorial.nix) demonstrates an independent module whose records and behaviour survive regeneration and an upgrade.

Generated Rust is a build artifact. Extend reusable business compositions in Nix and defined algorithms in Rust, with typed inputs, contract versions and regression tests. When a composition needs a new compiler construct, implement and validate that construct in the generator rather than adding a runtime interpreter. Keep those source changes under version control and merge platform updates there; editing generated handlers is not an extension strategy. Regeneration validates the composed model before writing artifacts.

Build outputs are:

- `outputs.generator`: the generator CLI.
- `outputs.publisher-generated`: standalone Rust sources, SQL, OpenAPI and dependency lock.
- `outputs.publisher`: the application binary used by the publisher process.

To export a Cargo project:

```sh
devenv shell -- cargo run -p openb2c -- --out generated examples/publisher.nix
```

Existing databases need reviewed migrations when their schema changes. Keep application data and operator-owned migrations outside immutable build outputs.

## Accept bank transfers

Customers pay through their own banks. Creating an order payment or subscription returns frozen receiving-account details, a GBP amount, a unique reference and an SVG QR code. The QR encodes readable instructions; it is not a universal banking-app payment link. The same details work without a browser. The simulated account uses zeroed account details and must never receive real money.

Bank transactions and allocations are separate from payment requests. The default Nix workflow automatically allocates a settled incoming transfer only when its account, reference, currency and full amount match a pending request. References ignore surrounding whitespace and letter case. The bank transaction is committed before fulfilment: a rejected workflow rolls back its allocations, access grants and accounting changes, retaining the receipt and a review reason. Repeated bank transaction IDs cannot create another receipt or grant access twice.

For local testing, an operator with `payments.import` can post a simulated bank fact to `/backoffice/transfers`:

```json
{
  "transaction_id": "local-transfer-1",
  "account": "simulated",
  "amount_minor": 1500,
  "currency": "GBP",
  "direction": "in",
  "status": "settled",
  "reference": "COPY_THE_REQUEST_REFERENCE",
  "settled_at": "2026-11-16T12:00:00Z",
  "updated_at": "2026-11-16T12:00:00Z"
}
```

Use actual test timestamps no later than the current clock. `GET /backoffice/transfers` requires `payments.reconcile` and shows allocations, remaining amounts, review reasons and feed status. Wrong references, underpayments, overpayments and transfers received after cancellation remain available for review. `/backoffice/transfers/{id}/allocate` accepts `payment_id` and `amount_minor`; partial receipts fulfil a request only once their allocations cover its full amount. `/reconcile` retries compiled matching. `/review` records an explicit `exclude` decision and a reason for unrelated transactions or corrections handled elsewhere; it never deletes the bank evidence. Financial and VAT reports flag unresolved transactions. Unallocated funds are not automatically classified or posted to the ledger.

Subscriptions keep a stable transfer reference across distinct renewal requests, suitable for a customer-managed standing order. At expiry the Nix workflow marks an unpaid renewal past due and access expires. Receipt of funds activates the paid period. Cancellation does not cancel the customer's standing order; the response reminds them to do that with their bank. Late or extra transfers remain recorded. There are no automatic charges or gateway retry endpoints.

A transfer-backed payment refund requires a separate confirmed outgoing transaction and explicit allocation to the original payment. No transfer is sent. A confirmed reversal of a single full incoming receipt applies the compiled refund/access reactions; reversals involving partial receipts require explicit review.

With `billing.payments.enable` (enabled by default with billing and payments), each allocation posts its actual amount from bank to customer deposits on the bank settlement date. Completion applies those deposits to the invoice without recording cash twice. Multiple partial receipts retain their separate dates; fully allocated, fulfilled requests leave the review queue. Closed-period cash postings remain durable billing events until an operator supplies an open posting date. Unallocated funds and reversed partial receipts still require review. Consumer opt-out uses `billing.payments.enable = false`; refunds and reversals for invoices already funded through deposits still update cash correctly.

Deposits crossing a VAT reporting boundary flag advance-payment tax points for review; invoice-based drafts do not automatically apportion advance-payment VAT. See [HMRC’s guidance on deposits and advance payments](https://www.gov.uk/guidance/vat-instalments-deposits-credit-sales).

Populated databases require a reviewed upgrade: existing invoice cash journals are preserved, while historical allocations without recorded cash are reconstructed from their bank evidence.

Bank readers implement a shared read-only Rust `BankFeed` interface. Nix selects one concrete adapter at compilation; generated services contain only that adapter. All adapters use the same durable facts, allocations, review queue, authorisation and compiled business reactions. Set `payments.feed` to `simulated` (the default), `open_banking` or `starling`. Real readers omit the simulated import endpoint and share `POST /backoffice/transfers/sync` with `payments.import` authority. No adapter initiates payments.

For UK Open Banking, select the reader in the business module:

```nix
payments.feed = "open_banking";
# Optional: only after verifying where this bank supplies the payer's reference.
# payments.openBanking.referenceField = "transaction_information";
```

Supply `OPEN_BANKING_BASE_URL` including the bank's versioned AIS prefix (for example `https://bank.example/open-banking/v4.0/aisp`), `BANK_ACCOUNT_ID`, `BANK_SYNC_FROM`, and verified `BANK_ACCOUNT_NAME`, `BANK_SORT_CODE` and `BANK_ACCOUNT_NUMBER`. Supply `OPEN_BANKING_ACCESS_TOKEN_FILE` containing a consent-authorised bearer token and `OPEN_BANKING_CLIENT_IDENTITY_FILE` containing its matching client certificate chain and private key in PEM. Credentials are reread each scan so a separate authorised client can rotate them. `OPEN_BANKING_CA_CERT_FILE` optionally adds a PEM CA certificate for a private or local test endpoint; certificate and hostname verification remain enabled. The reader uses HTTPS with mutual TLS, disables redirects, and calls the standard [`GET /accounts/{AccountId}/transactions`](https://openbankinguk.github.io/read-write-api-site3/v4.0/resources-and-data-models/aisp/Transactions.html), including `fromBookingDateTime` and `Links.Next` pagination. Pagination cannot leave the configured origin or account endpoint. v4 `BOOK`/`PDNG` and v3.1 `Booked`/`Pending` statuses are recognised. Only immutable booked transactions with stable IDs become cash evidence; pending transactions are skipped and mutable booked transactions or missing IDs fail the scan for review. GBP amounts are parsed exactly, without floating point.

Open Banking's `TransactionReference` may be a bank identifier, such as an FPID; `TransactionInformation` is an unstructured narrative. Neither is guaranteed to be the customer's reference. Both are retained as `source_reference` and `narrative` for operator review. The default `referenceField = "none"` retains transactions for manual allocation. Select `transaction_information` or `transaction_reference` only when that bank's documented behaviour supplies the exact payer reference in that field. This choice is compiled from Nix; the reader does not guess or extract substrings. Changed immutable monetary facts fail visibly rather than inventing a reversal.

Open Banking filters booking dates, not revisions. Its reader therefore replays the entire persisted `BANK_SYNC_FROM` window every 30 seconds, deduplicating stable IDs; it does not advance an update cursor. Choose a manageable reconciliation window covering the required history. Each scan is bounded to 100 pages, 50,000 transactions and 4 MB per page; a time budget also stops further page requests after 20 seconds. Incomplete scans preserve already committed facts, record an error and retry the original window. Explicit `/sync` accepts `changes_since` as the booking-window start for a one-off rescan; it does not change the worker's persisted window. This reader is not a statement-reconciliation service and cannot infer reversals from disappearing transactions.

Bank registration, access approval, consent acquisition/renewal and OAuth token refresh are not implemented here. They must be handled by an authorised AIS client or provider; supplying a URL alone does not grant access. See the standard's [Account and Transaction API Profile](https://openbankinguk.github.io/read-write-api-site3/v4.0/profiles/account-and-transaction-api-profile.html). The connector is an AIS resource reader, not a complete certified Open Banking client. There is no generic unsigned webhook endpoint; polling works without notifications.

Starling's account-holder API remains a separate adapter because it is not the standard AIS API. To compile it, set `payments.feed = "starling"` and rebuild. Supply `BANK_ACCOUNT_ID` (the account UID), `STARLING_CATEGORY_UID`, a read-only `STARLING_ACCESS_TOKEN`, `BANK_SYNC_FROM` within the last year, and the same verified receiving details. These shared names replace `STARLING_ACCOUNT_UID` and `STARLING_SYNC_FROM`. `STARLING_ENVIRONMENT` defaults to `sandbox`; production reads require selecting `production`. Keep the token in the business's own deployment. Starling prohibits collecting other customers' personal access tokens; third-party account access has separate approval requirements. See its [API documentation](https://developer.starlingbank.com/docs) and [access rules](https://developer.starlingbank.com/faq).

Starling polls every 30 seconds and advances its changes-since cursor only after a complete successful read, with a two-minute overlap. Optional V2 feed webhooks use the base URL `/backoffice/transfers` and `STARLING_WEBHOOK_PUBLIC_KEY` (PEM or base64 DER). RSA/SHA-512 signatures cover the original bytes. Notifications request an earlier read; facts always come from the configured account's API. Responses over 4 MB fail without advancing the cursor. Review older history explicitly after prolonged outages.

The generation test `generated_publisher_runs_end_to_end_against_local_ais` builds the publisher with the AIS adapter and runs its REST interface against a localhost HTTPS bank simulator. The simulator requires mutual TLS and a bearer token. It exercises signup, payment instructions, paginated receipts, protected downloads, interrupted scans, restart/replay, credential rotation, conflicting facts, manual allocation, refunds, balanced journals, rejected fulfilment and subscription activation/cancellation. A generated-service probe then checks renewal and download expiry at the exact period boundary using explicit test clock readings. Run it with `devenv shell -- cargo test -p openb2c --test generation generated_publisher_runs_end_to_end_against_local_ais`. Each run creates its own database and certificates; it does not use your configured bank or publisher data. Bank protocols and signed Starling notifications are also tested locally. No live bank connection or real payment has been exercised. Existing fake-payment history remains readable for reviewed upgrades; it is never converted into real bank receipts automatically. The payment contract is version 2 and populated databases need reviewed SQL for additional fields, tables and the expanded bank-source enum. Switching readers is an operator-reviewed migration, not an automatic conversion of transaction identities or sync state; use one source for an account's historical money to avoid recording the same funds through two adapters.

## Run the business

The publisher also enables `accounting`, `billing`, `documents`, `purchasing`, `banking` and `tax`. These are independently selectable: accounting needs no products or customers, billing consumes accounting, purchasing consumes accounting and documents, and banking/tax consume accounting. Billing automatically consumes paid orders and subscription payments; opt out with `billing.orders.enable = false` or `billing.subscriptions.enable = false`.

Internal REST operations live under `/backoffice/` and require explicit operator scopes such as `accounting.read`, `accounting.write`, `accounting.close`, `billing.read`, `billing.write`, `billing.refund`, `documents.read`, `documents.write`, `purchasing.read`, `purchasing.write`, `banking.read`, `banking.write` and `tax.read`. Customer sessions receive none of these. Use `/openapi.json` for request formats.

- Accounting posts balanced GBP journals, reverses corrections, locks closed periods, and reports a trial balance, profit and loss, and balance sheet.
- Billing issues invoices, records full receipts and credit notes, and defers subscription revenue until explicitly recognised daily on a pro-rata basis with `/backoffice/revenue/recognize`. A credit note reverses the sale and recognised revenue without implying a cash movement. Record a confirmed cash refund separately through `/backoffice/invoices/{id}/refund` with `billing.refund`. A fully allocated bank refund or reversal performs both operations; access cancellation alone implies neither. New receipts debit `bank`; historical refunds use the asset account on the original receipt.
- Documents store immutable base64-encoded evidence and SHA-256 digests in SQLite (375 kB maximum per document). Purchasing records suppliers and receipt-backed expenses, full payments and unpaid expense reversals. Recording a payment does not initiate a bank transfer.
- Banking imports statement lines through REST and matches each to one ledger line with the same account, currency and signed amount. Bank-feed ingestion and ledger reconciliation remain distinct; fees and unrelated funds need explicit accounting entries. The old `clearing` account remains available for historical records.

VAT support targets UK domestic GBP transactions under standard accrual accounting. Explicit treatments are `standard` (20%), `reduced` (5%), `zero`, `exempt`, `outside`, `not_registered` and `unassessed`; amounts and treatment are preserved on source records and credits. Manual invoices/expenses accept net amounts; automatic sales use VAT-inclusive catalog totals. Recoverable input VAT must be explicitly selected on expenses and requires `tax.vat.registration = "registered"`.

The example leaves registration and sales treatment unassessed. Set `tax.vat.registration` to `registered` or `not_registered`, and set `billing.taxCode` to the appropriate uniform sales treatment before assessing its reports. A mixed-treatment catalog needs line-specific tax support, which is not implemented. `/backoffice/reports/vat` produces draft HMRC box totals with source evidence and review blockers; corrections appear in their posting period. [HMRC's VAT Return guidance](https://www.gov.uk/guidance/how-to-fill-in-and-submit-your-vat-return-vat-notice-70012) defines the return boxes.

These are internal financial records, not a complete statutory invoicing or tax-filing system. HMRC submission/MTD authorisation, Corporation Tax, payroll, imports/reverse charge, Northern Ireland/EU trade, special VAT schemes, supplier refunds and partial settlements are not implemented. VAT reports always require review, and unassessed treatments or unclassified manual financial postings are flagged. Reviewed billing activation reconstructs historical sales from durable payment events with tax marked unassessed, rather than applying today's tax policy retrospectively. Missing source evidence blocks the upgrade.

## Operate and upgrade

Keep the model, business modules, platform source revision, build artifacts and operator-owned SQL migrations under your control. Place data and numbered migrations outside generated outputs; set `DB_PATH` and `MIGRATIONS_DIR` consistently for server and operator commands. No new business capabilities should be added until the ownership regression scenarios continue to pass.

Closed periods reject backdated journals. Invoices and expenses can preserve their original `date` while supplying an open `posting_date`. Accepted payment/refund events whose accounting period is closed remain durable under `/backoffice/billing-events`; `POST /backoffice/billing-events/post` with an open `date` posts them in order. Reports flag unposted events, and revenue recognition waits for them. VAT drafts retain source dates and posting dates for review; they do not determine how a previously filed return must be corrected.

Stop the server before recovery or upgrading. With a newly built application binary, inspect the database and take a verified snapshot **before opening it under the new model**:

```sh
"$app/bin/openb2c-app" --doctor
"$app/bin/openb2c-app" --backup /backups/before-upgrade.sqlite > /backups/before-upgrade.json
"$app/bin/openb2c-app" --plan-upgrade > /tmp/openb2c-upgrade.json
"$app/bin/openb2c-app" --approve-upgrade "$(jq -r .plan_hash /tmp/openb2c-upgrade.json)" 'Reviewed business transition and SQL'
"$app/bin/openb2c-app" --upgrade
```

The review binds the target model, existing data and actual SQL migration files; changes invalidate approval. Outstanding obligations block capability retirement. Existing tables remain as archives, role renames must preserve their records, and failed upgrades roll back SQL, data changes and the model snapshot together. An old server process refuses requests after the stored model changes. Consumer opt-outs stop future integrations while retained billing/access consumers still handle corrections to existing records. Contract or schema changes can require reviewed, numbered SQL; approval does not bypass structural checks. Migration files must not contain transaction-control statements.

Backups use [SQLite's consistent snapshot mechanism](https://www.sqlite.org/lang_vacuum.html#vacuuminto), include committed WAL data, embedded models, credentials, documents, audit history and numbered SQL files, and are created with owner-only permissions. Retain the reported SHA-256 independently and keep matching source/build artifacts and deployment secrets. Restore into **new** database and migration paths; existing data is never overwritten:

```sh
DB_PATH=/restore/publisher.sqlite MIGRATIONS_DIR=/restore/migrations \
  "$app/bin/openb2c-app" --restore /backups/before-upgrade.sqlite \
  "$(jq -r .sha256 /backups/before-upgrade.json)"
```

Run `--doctor`, then the reviewed upgrade workflow against those restored paths before starting the service. `--reset-password EMAIL --password-stdin` replaces a lost password, revokes all account sessions and API keys, and records recovery in the audit log. It requires local database ownership; there is no anonymous recovery endpoint. Staff membership and permissions remain separate from password recovery.

## Deliver publications

`downloads.enable` adds immutable file storage and delivery. It consumes entitlements and works without a catalog, orders or subscriptions; those capabilities supply entitlements when selected. Financial evidence documents remain separate from customer publications.

An operator with `downloads.publish` uploads a file and explicitly chooses the entitlement resources that can open it. In the example these resources are the product names `digital-book` and `reader-membership`. Either is sufficient; publishing content never grants access itself. With an operator bearer credential in `OPERATOR_TOKEN`:

```sh
base64 -w0 handbook.pdf |
  jq -Rs '{idempotency_key:"handbook-v1",name:"handbook.pdf",
    media_type:"application/pdf",content_base64:.,
    resources:["digital-book","reader-membership"]}' |
  curl -fsS http://127.0.0.1:3085/b2c/downloads \
    -H "Authorization: Bearer $OPERATOR_TOKEN" \
    -H 'Content-Type: application/json' --data-binary @-
```

Files contain up to 8 MiB decoded bytes. The default HTTP body limit is 1 MiB; start the server with `MAX_REQUEST_BODY_BYTES=12000000` for uploads up to the file limit. Filenames use ASCII letters, digits, spaces, dots, underscores and hyphens, with no leading dot or paths. Supply a media type without parameters. Upload keys bind the bytes, filename, media type and access requirements; changed content needs a new key. Files live in SQLite and are included in the ordinary backup.

After a simulated successful purchase or subscription payment, a customer uses their bearer credential to list and retrieve available publications:

```sh
curl -fsS http://127.0.0.1:3085/b2c/downloads \
  -H "Authorization: Bearer $CUSTOMER_TOKEN"
curl -fsS http://127.0.0.1:3085/b2c/downloads/1/content \
  -H "Authorization: Bearer $CUSTOMER_TOKEN" -o handbook.pdf
```

Delivery returns actual bytes with the stored media type, an attachment filename, a SHA-256 ETag and `Cache-Control: private, no-store`. Every request checks current access; refunds, immediate cancellation and expiry deny subsequent downloads unless another active entitlement still covers the file. Cancellation at period end preserves access until that boundary. Files already saved by a customer cannot be recalled.

Operators list publication metadata with `GET /b2c/downloads/manage` and withdraw files with `POST /b2c/downloads/{id}/withdraw` and an empty JSON object. Withdrawal is permanent for that file ID, retains the data and audit history, and does not issue refunds. An upload replay does not republish it. Generic table reads/writes are disabled; operator publication permissions alone do not grant customer download access. Storage is bounded and buffered for this pilot; streaming, resumable downloads, object storage and content version management are not implemented.

## Trial the business

Use an isolated database and simulated bank transfers for a sustained trial. This is a review procedure; automated tests do not replace business-owner or accountant review.

1. Publish representative books, create separate customer and staff accounts, and try both a book purchase and a reader subscription. Verify file bytes, unauthorized requests, refund/replay behavior, cancellation, expiry and failed-renewal recovery. Keep overlapping purchase and subscription access in the trial.
2. Record a representative trading period: supplier evidence, expenses, settlements, fees, a credit note without a refund, an eventual refund, deferred subscription revenue and bank reconciliation. Review tax classification before closing; then record a late event into an open posting period.
3. Take routine backups with the local CLI, retain the returned hashes separately, and restore into new paths. Compare reports and downloaded file hashes, exercise credential recovery, and rehearse a reviewed upgrade before changing the running business.
4. Have the business owner assess the complete workflows and a UK accountant assess the invoices, journals, reconciliations and VAT drafts against the trial's expected results. Record unresolved issues before considering live operation. Real collection and HMRC filing remain outside this pilot.

## Development

```sh
devenv test
```

The ownership suite covers a full trading period, late postings, corrections, retirement blockers, stale approvals, legacy credentials/refunds, application-owned workflows, and backup/restore.

The generation suite also builds and runs exported support crates with reference execution disabled. It verifies that Nix overrides change executable business behavior, failing reactions roll back the initiating transaction, custom operations compile their guards/aggregates/cascades, incompatible models are rejected, and partial selections omit unrelated implementations.

Download tests cover partial compositions, renamed table roles, immutable uploads, purchase/refund and subscription expiry, overlapping grants, withdrawal, authorization, actual REST bytes, corruption detection and restoration from backup.

The root Devenv environment provides Rust, formatting and Clippy hooks, workspace tests, build outputs and the publisher process. Rust packages build through `languages.rust.import`.

`nix/model/` contains the modelling system; `nix/build/` contains packaging and generation helpers. `crates/` contains the schema compiler, generator and runtime. Tests and option definitions describe the detailed behavior.

The pilot is backend-only. Live payment collection, shipping, inventory and proration are not implemented.

Licensed under [GPL-3.0](LICENSE.md).
