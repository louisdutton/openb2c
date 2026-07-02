import { describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { genEffectsInterface } from "./effects";
import { genMcpServer } from "./mcp";
import { genOpenAPI } from "./openapi";
import { genRuntime } from "./runtime";
import { genRoutes } from "./server";
import { genServices } from "./services";
import { genSQL } from "./sql";
import { genTypes } from "./typescript";
import type { Schema } from "./types";
import { getEcommerceConfig } from "./utils";

const PROJECT_ROOT = join(import.meta.dir, "..", "..");

async function nixEvalJson(args: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  const proc = Bun.spawn(["nix", "eval", "--impure", "--json", ...args], {
    cwd: PROJECT_ROOT,
    stdout: "pipe",
    stderr: "pipe",
  });

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);

  return { stdout, stderr, exitCode };
}

async function loadDuchyOperaSchema(): Promise<Schema> {
  const result = await nixEvalJson(["-f", join(PROJECT_ROOT, "examples", "duchyopera", "composition.nix")]);
  if (result.exitCode !== 0) {
    throw new Error(`nix eval failed for duchyopera: ${result.stderr}`);
  }
  return JSON.parse(result.stdout) as Schema;
}

function writeGenerated(schema: Schema): string {
  const dir = mkdtempSync(join(tmpdir(), "openb2c-duchy-commerce-"));
  writeFileSync(join(dir, "schema.sql"), genSQL(schema.tables, schema.constraints));
  writeFileSync(join(dir, "types.ts"), genTypes(schema.tables, schema.operations));
  writeFileSync(join(dir, "services.ts"), genServices(schema));
  writeFileSync(join(dir, "runtime.ts"), genRuntime(schema));
  writeFileSync(join(dir, "effects.ts"), genEffectsInterface(schema));
  writeFileSync(join(dir, "server.ts"), genRoutes(schema));
  return dir;
}

function seedDuchyOpera(dbPath: string, options: { venueCapacity?: number } = {}) {
  const venueCapacity = options.venueCapacity ?? 900;
  const db = new Database(dbPath);
  try {
    db.query("INSERT INTO user (id, email, name) VALUES (1, 'ada@example.test', 'Ada Lovelace')").run();
    db.query("INSERT INTO venue (id, name, address, city, postcode, capacity) VALUES (1, 'Hall for Cornwall', 'Back Quay', 'Truro', 'TR1 2LL', ?)").run(venueCapacity);
    db.query("INSERT INTO production (id, name, season, status) VALUES (1, 'The Magic Flute', 'Summer 2026', 'active')").run();
    db.query("INSERT INTO performance (id, production_id, venue_id, starts_at, ends_at, price_pence, status) VALUES (1, 1, 1, '2026-06-27T19:30:00Z', '2026-06-27T22:00:00Z', 2500, 'scheduled')").run();
    db.query("INSERT INTO performance (id, production_id, venue_id, starts_at, ends_at, price_pence, status) VALUES (2, 1, 1, '2026-06-28T19:30:00Z', '2026-06-28T21:30:00Z', 3000, 'cancelled')").run();
  } finally {
    db.close();
  }
}

function applyDuchySchema(schema: Schema, dbPath: string, options: { venueCapacity?: number } = {}) {
  const db = new Database(dbPath);
  try {
    for (const stmt of genSQL(schema.tables, schema.constraints).split(/;\s*\n/).filter(s => s.trim())) db.run(stmt);
  } finally {
    db.close();
  }
  seedDuchyOpera(dbPath, options);
}

async function hmacSha256Hex(secret: string, body: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const signature = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(body));
  return Array.from(new Uint8Array(signature)).map(b => b.toString(16).padStart(2, "0")).join("");
}

function clearEnv() {
  delete process.env.DB_PATH;
  delete process.env.PORT;
  delete process.env.AUTH_ENABLED;
  delete process.env.PAYMENT_WEBHOOK_SECRET;
  delete process.env.EMAIL_WEBHOOK_URL;
  delete process.env.PAYMENT_PROVIDER;
  delete process.env.PAYMENT_API_KEY;
  delete process.env.STRIPE_SECRET_KEY;
  delete process.env.STRIPE_API_BASE;
}

function genericShopSchema(): Schema {
  return {
    organization: { name: "Generic Shop", description: "Generic ecommerce test" },
    tables: {
      user: {
        id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
      },
      product: {
        id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
        name: { type: "text", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        price_pence: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        status: { type: "text", pk: false, auto: false, required: true, unique: false, default: "'available'", references: null, constraints: { enum: ["available", "unavailable"] } },
      },
      cart_order: {
        id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
        user_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "user(id)" },
        status: { type: "text", pk: false, auto: false, required: true, unique: false, default: "'checkout_pending'", references: null, constraints: { enum: ["checkout_pending", "payment_pending", "paid", "expired", "cancelled", "refund_required", "refunded"] } },
        amount_pence: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        currency: { type: "text", pk: false, auto: false, required: true, unique: false, default: "'GBP'", references: null },
        expires_at: { type: "text", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        payment_deadline_at: { type: "text", pk: false, auto: false, required: false, unique: false, default: null, references: null },
        payment_reference: { type: "text", pk: false, auto: false, required: false, unique: false, default: null, references: null },
        client: { type: "text", pk: false, auto: false, required: false, unique: false, default: "'web'", references: null },
      },
      cart_line: {
        id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
        product_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "product(id)" },
        user_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "user(id)" },
        price_pence: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        status: { type: "text", pk: false, auto: false, required: true, unique: false, default: "'reserved'", references: null, constraints: { enum: ["reserved", "fulfilled", "cancelled"] } },
        colour: { type: "text", pk: false, auto: false, required: false, unique: false, default: null, references: null, constraints: { enum: ["black", "white"] } },
      },
      cart_order_line: {
        id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
        cart_order_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "cart_order(id)" },
        cart_line_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "cart_line(id)" },
      },
      payment: {
        id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
        user_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "user(id)" },
        amount_pence: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        status: { type: "text", pk: false, auto: false, required: true, unique: false, default: "'pending'", references: null, constraints: { enum: ["pending", "authorized", "completed", "failed", "cancelled", "refund_required", "refunded"] } },
        reference: { type: "text", pk: false, auto: false, required: true, unique: true, default: null, references: null },
        client: { type: "text", pk: false, auto: false, required: false, unique: false, default: "'web'", references: null },
      },
      payment_line: {
        id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
        payment_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "payment(id)" },
        cart_line_id: { type: "integer", pk: false, auto: false, required: true, unique: false, default: null, references: "cart_line(id)" },
      },
    },
    operations: {},
    commerce: {
      enabled: true,
      checkout: { currency: "GBP", expiryMinutes: 15, paymentDeadlineMinutes: 30, allowAsyncPayments: false, maxQuantity: 20, maxLines: 50 },
    },
  };
}

describe("Duchy Opera commerce workflow", () => {
  test("generic ecommerce generation does not expose booking-specific compatibility aliases", () => {
    const schema = genericShopSchema();
    const services = genServices(schema);
    const routes = genRoutes(schema);
    const mcp = genMcpServer(schema);
    const openapi = JSON.parse(genOpenAPI(schema));

    expect(services).toContain("checkoutCommerceCart");
    expect(services).toContain("createCommercePaymentSession");
    expect(services).not.toContain("ReserveBookingInput");
    expect(routes).toContain("/commerce/checkout");
    expect(routes).toContain('path: "/commerce/catalog"');
    expect(routes).toContain("/commerce/orders/:id/payment-session");
    expect(routes).toContain("/commerce/payments/local-simulator");
    expect(routes).not.toContain("/commerce/orders/:id/payment-intent");
    expect(routes).not.toContain("/commerce/bookings/reserve");
    expect(routes).toContain('path: "/auth/context"');
    expect(routes).toContain('url.pathname.startsWith("/auth/")');
    expect(mcp).toContain("checkout_cart");
    expect(mcp).toContain("list_commerce_catalog");
    expect(mcp).toContain("create_order_payment_session");
    expect(mcp).not.toContain("reserve_booking");
    expect(openapi.paths["/commerce/checkout"]).toBeDefined();
    expect(openapi.paths["/commerce/catalog"]).toBeDefined();
    expect(openapi.paths["/commerce/orders/{id}/payment-session"]).toBeDefined();
    expect(openapi.paths["/commerce/orders/{id}/payment-intent"]).toBeUndefined();
    expect(openapi.paths["/commerce/bookings/reserve"]).toBeUndefined();
    expect(openapi.paths["/auth/context"]).toBeDefined();
    expect(openapi.components.securitySchemes.bearerAuth).toMatchObject({ type: "http", scheme: "bearer" });
    expect(openapi.components.securitySchemes.certificateAuth).toMatchObject({ type: "apiKey", name: "X-Certificate" });
    expect(openapi.paths["/commerce/catalog"].get.security).toBeUndefined();
    expect(openapi.paths["/commerce/checkout"].post.security).toEqual([
      { bearerAuth: [] },
      { certificateAuth: [], certificateSignature: [], certificateTimestamp: [] },
    ]);
    expect(openapi.paths["/commerce/payments/webhook"].post.security).toEqual([{ paymentWebhookSignature: [] }]);
    expect(openapi.components.schemas.CommerceCheckoutInput).toMatchObject({
      additionalProperties: false,
      properties: {
        items: { minItems: 1, maxItems: 50 },
      },
    });
    expect(openapi.components.schemas.CommerceCartItemInput).toMatchObject({
      additionalProperties: false,
      properties: {
        quantity: { minimum: 1, maximum: 20 },
        options: { $ref: "#/components/schemas/CommerceCartItemOptions" },
      },
    });
    expect(openapi.components.schemas.CommerceCartItemOptions).toMatchObject({
      additionalProperties: false,
      properties: {
        colour: { type: "string", enum: ["black", "white"], nullable: true },
      },
    });
  });

  test("Duchy Opera commerce does not expose booking aliases", async () => {
    const schema = await loadDuchyOperaSchema();
    const services = genServices(schema);
    const routes = genRoutes(schema);
    const mcp = genMcpServer(schema);
    const effects = genEffectsInterface(schema);
    const openapi = JSON.parse(genOpenAPI(schema));

    expect(services).not.toContain("ReserveBookingInput");
    expect(routes).not.toContain("/commerce/bookings/reserve");
    expect(mcp).not.toContain("reserve_booking");
    expect(effects).not.toContain('"booking.reserved"');
    expect(openapi.paths["/commerce/bookings/reserve"]).toBeUndefined();
    expect(openapi.components.schemas.CommerceCartItemOptions.properties.ticket_type).toMatchObject({
      type: "string",
      enum: ["standard", "concession", "patron", "vip"],
      default: "standard",
    });
    expect(openapi.components.schemas.CommerceCartItemOptions.properties.seat).toBeUndefined();
  });

  test("commerce browser UI derives checkout user from auth context", async () => {
    const commerceUi = await Bun.file(join(PROJECT_ROOT, "schema", "ui", "components", "ob-commerce.ts")).text();
    const authMenuUi = await Bun.file(join(PROJECT_ROOT, "schema", "ui", "components", "ob-auth-menu.ts")).text();
    const authPanelUi = await Bun.file(join(PROJECT_ROOT, "schema", "ui", "components", "ob-auth-panel.ts")).text();
    const routeUi = await Bun.file(join(PROJECT_ROOT, "schema", "ui", "components", "ob-route-outlet.ts")).text();
    const appUi = await Bun.file(join(PROJECT_ROOT, "schema", "ui", "components", "ob-app.ts")).text();
    const apiUi = await Bun.file(join(PROJECT_ROOT, "schema", "ui", "components", "ob-api.ts")).text();

    expect(commerceUi).toContain("authContext.userId");
    expect(commerceUi).toContain('request("/commerce/catalog")');
    expect(commerceUi).not.toContain("/api/${entity}s?limit=200");
    expect(commerceUi).toContain("ob-auth-required");
    expect(commerceUi).toContain("Sign in to continue");
    expect(commerceUi).toContain('returnTo: "/cart"');
    expect(commerceUi).toContain("sessionStorage");
    expect(commerceUi).not.toContain('inputmode="email"');
    expect(commerceUi).not.toContain("checkout-customer");
    expect(commerceUi).not.toContain("Select a customer");
    expect(commerceUi).not.toContain("body.user_id");
    expect(commerceUi).not.toContain("Response JSON");
    expect(commerceUi).not.toContain("_renderLinks");
    expect(commerceUi).not.toContain("data-form=\"signin\"");
    expect(authMenuUi).toContain("#/login");
    expect(authMenuUi).not.toContain('inputmode="email"');
    expect(authPanelUi).toContain('inputmode="email"');
    expect(authPanelUi).toContain("setCertificateAuth");
    expect(authPanelUi).toContain("clearAuthContext");
    expect(routeUi).toContain("./ob-auth-page");
    expect(appUi).toContain("<ob-auth-menu>");
    expect(apiUi).toContain("hasIdentityAuth");
    expect(apiUi).toContain("setCertificateAuth");
    expect(apiUi).toContain("X-Certificate");
  });

  test("checks out a cart, creates a payment session, confirms paid orders, and expires stale checkouts", async () => {
    const schema = await loadDuchyOperaSchema();
    const commerce = getEcommerceConfig(schema);
    expect(commerce?.enabled).toBe(true);
    expect(commerce?.catalog.entity).toBe("performance");
    expect(commerce?.order.entity).toBe("booking");
    expect(commerce?.lineItem.entity).toBe("ticket");
    expect(commerce?.catalog.capacity).toMatchObject({
      field: { table: "venue", field: "capacity" },
      via: { table: "performance", field: "venue_id" },
    });
    expect(Object.keys(commerce?.lineItem.options || {})).toEqual(["ticket_type"]);
    expect(commerce?.lineItem.options.ticket_type.choices).toEqual(["standard", "concession", "patron", "vip"]);

    const dir = writeGenerated(schema);
    const dbPath = join(dir, "duchy-commerce.sqlite");
    process.env.DB_PATH = dbPath;
    process.env.PORT = "0";
    process.env.AUTH_ENABLED = "false";
    process.env.PAYMENT_WEBHOOK_SECRET = "test-webhook-secret";
    const receiptEmails: unknown[] = [];
    const email = Bun.serve({
      port: 0,
      async fetch(req) {
        receiptEmails.push(await req.json());
        return Response.json({ ok: true });
      },
    });
    process.env.EMAIL_WEBHOOK_URL = `http://127.0.0.1:${email.port}`;

    const { server } = await import(pathToFileURL(join(dir, "server.ts")).href);
    const services = await import(pathToFileURL(join(dir, "services.ts")).href);
    const base = `http://127.0.0.1:${server.port}`;
    seedDuchyOpera(dbPath);

    try {
      const catalog = await fetch(`${base}/commerce/catalog`);
      expect(catalog.status).toBe(200);
      const catalogBody = await catalog.json() as {
        items: { id: number; production_id: number }[];
        lookups: Record<string, Record<string, string>>;
      };
      expect(catalogBody.items.map(item => item.production_id)).toEqual([1]);
      expect(catalogBody.lookups.production_id["1"]).toBe("The Magic Flute");
      expect(catalogBody.lookups.venue_id["1"]).toBe("Hall for Cornwall");

      const directDb = new Database(dbPath);
      try {
        const directCheckout = services.checkoutCommerceCart(directDb, {
          client: "web",
          items: [{ item_id: 1, quantity: 1, options: { ticket_type: "standard" } }],
        }, { userId: 1, scopes: ["*"] });
        expect(directCheckout.ok).toBe(true);
      } finally {
        directDb.close();
      }

      const checkout = await fetch(`${base}/commerce/checkout`, {
        method: "POST",
        headers: { "content-type": "application/json", "Idempotency-Key": "checkout-opera-1" },
        body: JSON.stringify({
          user_id: 1,
          client: "web",
          items: [
            {
              item_id: 1,
              quantity: 2,
              options: { ticket_type: "standard" },
            },
          ],
        }),
      });
      expect(checkout.status).toBe(201);
      const checkedOut = await checkout.json() as { order_id: number; line_item_ids: number[]; amount_pence: number; status: string };
      expect(checkedOut.line_item_ids).toHaveLength(2);
      expect(checkedOut.amount_pence).toBe(5000);
      expect(checkedOut.status).toBe("checkout_pending");

      const intent = await fetch(`${base}/commerce/orders/${checkedOut.order_id}/payment-session`, {
        method: "POST",
        headers: { "Idempotency-Key": "session-opera-1" },
      });
      expect(intent.status).toBe(201);
      const payment = await intent.json() as {
        order_id: number;
        transaction_id: number;
        reference: string;
        amount_pence: number;
        provider: string;
        status: string;
        payment_deadline_at: string;
        next_action: { type: string; url: string };
      };
      expect(payment.order_id).toBe(checkedOut.order_id);
      expect(payment.reference).toStartWith("fake_ps_");
      expect(payment.amount_pence).toBe(5000);
      expect(payment.provider).toBe("local");
      expect(payment.status).toBe("payment_pending");
      expect(Date.parse(payment.payment_deadline_at)).toBeGreaterThan(Date.now());
      expect(payment.next_action.type).toBe("redirect");
      const paymentActionUrl = String(payment.next_action.url);
      expect(paymentActionUrl).toContain("/commerce/payments/local-simulator");
      expect(paymentActionUrl).toContain(encodeURIComponent(payment.reference));

      const simulator = await fetch(new URL(paymentActionUrl, base));
      expect(simulator.status).toBe(200);
      const simulatorHtml = await simulator.text();
      const actionMatch = simulatorHtml.match(/const actions = (.*);/);
      expect(actionMatch).toBeTruthy();
      const actions = JSON.parse(actionMatch![1]) as { status: string; body: string; signature: string }[];
      const authorized = actions.find(action => action.status === "authorized");
      expect(authorized).toBeTruthy();
      expect(authorized!.signature).toBe(`sha256=${await hmacSha256Hex("test-webhook-secret", authorized!.body)}`);
      const webhook = await fetch(`${base}/commerce/payments/webhook`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "X-OpenB2C-Signature": authorized!.signature,
        },
        body: authorized!.body,
      });
      expect(webhook.status).toBe(200);
      expect(await webhook.json()).toMatchObject({
        order_id: checkedOut.order_id,
        transaction_id: payment.transaction_id,
        status: "paid",
        idempotent: false,
      });
      const duplicateWebhook = await fetch(`${base}/commerce/payments/webhook`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "X-OpenB2C-Signature": authorized!.signature,
        },
        body: authorized!.body,
      });
      expect(duplicateWebhook.status).toBe(200);
      expect(await duplicateWebhook.json()).toMatchObject({ status: "paid", idempotent: true });
      expect(receiptEmails).toHaveLength(1);
      expect(receiptEmails[0]).toMatchObject({
        channel: "email",
        template: "receipt",
        to: "customer",
        data: {
          order_id: checkedOut.order_id,
          transaction_id: payment.transaction_id,
          reference: payment.reference,
          amount_pence: 5000,
          currency: "GBP",
          customer: { id: 1, email: "ada@example.test", name: "Ada Lovelace" },
          line_item_ids: checkedOut.line_item_ids,
          ticket_ids: checkedOut.line_item_ids,
          line_items: [
            {
              id: checkedOut.line_item_ids[0],
              catalog_item_id: 1,
              catalog_label: "The Magic Flute",
              quantity: 1,
              unit_amount_pence: 2500,
              amount_pence: 2500,
              options: { ticket_type: "standard" },
            },
            {
              id: checkedOut.line_item_ids[1],
              catalog_item_id: 1,
              catalog_label: "The Magic Flute",
              quantity: 1,
              unit_amount_pence: 2500,
              amount_pence: 2500,
              options: { ticket_type: "standard" },
            },
          ],
        },
      });

      const db = new Database(dbPath);
      try {
        const booking = db.query<{ status: string }, []>("SELECT status FROM booking WHERE id = ?").get(checkedOut.order_id);
        const transaction = db.query<{ status: string }, []>("SELECT status FROM [transaction] WHERE id = ?").get(payment.transaction_id);
        const tickets = db.query<{ status: string }, []>("SELECT status FROM ticket WHERE id IN (SELECT ticket_id FROM booking_ticket WHERE booking_id = ?) ORDER BY id").all(checkedOut.order_id);
        expect(booking?.status).toBe("paid");
        expect(transaction?.status).toBe("completed");
        expect(tickets.map(ticket => ticket.status)).toEqual(["confirmed", "confirmed"]);
      } finally {
        db.close();
      }

      const staleCheckout = await fetch(`${base}/commerce/checkout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ user_id: 1, items: [{ item_id: 1, quantity: 1 }] }),
      });
      expect(staleCheckout.status).toBe(201);
      const stale = await staleCheckout.json() as { order_id: number; line_item_ids: number[] };

      const staleDb = new Database(dbPath);
      try {
        staleDb.query("UPDATE booking SET expires_at = ? WHERE id = ?").run(new Date(Date.now() - 60000).toISOString(), stale.order_id);
      } finally {
        staleDb.close();
      }

      const expire = await fetch(`${base}/commerce/orders/expire`, { method: "POST" });
      expect(expire.status).toBe(200);
      expect(await expire.json()).toEqual({ expired: 1 });

      const finalDb = new Database(dbPath);
      try {
        const expiredBooking = finalDb.query<{ status: string }, []>("SELECT status FROM booking WHERE id = ?").get(stale.order_id);
        const expiredTicket = finalDb.query<{ status: string }, []>("SELECT status FROM ticket WHERE id = ?").get(stale.line_item_ids[0]);
        const effects = finalDb.query<{ status: string }, []>("SELECT status FROM openb2c_effect_attempt ORDER BY id").all();
        expect(expiredBooking?.status).toBe("expired");
        expect(expiredTicket?.status).toBe("cancelled");
        expect(effects.length).toBeGreaterThanOrEqual(5);
        expect(effects.every(effect => effect.status === "succeeded")).toBe(true);
      } finally {
        finalDb.close();
      }
    } finally {
      server.stop(true);
      email.stop(true);
      clearEnv();
    }
  });

  test("enforces inferred catalog capacity during checkout", async () => {
    const schema = await loadDuchyOperaSchema();
    const dir = writeGenerated(schema);
    const dbPath = join(dir, "duchy-capacity-commerce.sqlite");
    process.env.DB_PATH = dbPath;
    process.env.PORT = "0";
    process.env.AUTH_ENABLED = "false";

    const { server } = await import(`${pathToFileURL(join(dir, "server.ts")).href}?capacity=${Date.now()}`);
    const base = `http://127.0.0.1:${server.port}`;
    seedDuchyOpera(dbPath, { venueCapacity: 2 });

    try {
      const first = await fetch(`${base}/commerce/checkout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          user_id: 1,
          items: [{ item_id: 1, quantity: 2, options: { ticket_type: "standard" } }],
        }),
      });
      expect(first.status).toBe(201);
      const firstOrder = await first.json() as { order_id: number };
      const firstSession = await fetch(`${base}/commerce/orders/${firstOrder.order_id}/payment-session`, { method: "POST" });
      expect(firstSession.status).toBe(201);

      const oversell = await fetch(`${base}/commerce/checkout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          user_id: 1,
          items: [{ item_id: 1, quantity: 1, options: { ticket_type: "standard" } }],
        }),
      });
      expect(oversell.status).toBe(409);
      expect(await oversell.json()).toMatchObject({
        error: "not enough capacity available",
        code: "bad_state",
        details: {
          item_id: "1",
          capacity: "2",
          allocated: "2",
          requested: "1",
          available: "0",
        },
      });

      const db = new Database(dbPath);
      try {
        db.query("UPDATE booking SET expires_at = ? WHERE id = ?").run(new Date(Date.now() - 60000).toISOString(), firstOrder.order_id);
      } finally {
        db.close();
      }

      const stillLocked = await fetch(`${base}/commerce/checkout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          user_id: 1,
          items: [{ item_id: 1, quantity: 1, options: { ticket_type: "standard" } }],
        }),
      });
      expect(stillLocked.status).toBe(409);

      const deadlineDb = new Database(dbPath);
      try {
        deadlineDb.query("UPDATE booking SET payment_deadline_at = ? WHERE id = ?").run(new Date(Date.now() - 60000).toISOString(), firstOrder.order_id);
      } finally {
        deadlineDb.close();
      }

      const expire = await fetch(`${base}/commerce/orders/expire`, { method: "POST" });
      expect(expire.status).toBe(200);
      expect(await expire.json()).toEqual({ expired: 1 });

      const afterExpiry = await fetch(`${base}/commerce/checkout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          user_id: 1,
          items: [{ item_id: 1, quantity: 1, options: { ticket_type: "standard" } }],
        }),
      });
      expect(afterExpiry.status).toBe(201);
    } finally {
      server.stop(true);
      clearEnv();
    }
  });

  test("late authorized payment cancels when capacity has gone", async () => {
    const schema = await loadDuchyOperaSchema();
    const dir = writeGenerated(schema);
    const services = await import(`${pathToFileURL(join(dir, "services.ts")).href}?late-auth=${Date.now()}`);
    const dbPath = join(dir, "duchy-late-auth.sqlite");
    applyDuchySchema(schema, dbPath, { venueCapacity: 2 });
    const db = new Database(dbPath);
    const auth = { userId: null, scopes: ["*"] };
    process.env.PAYMENT_PROVIDER = "fake";

    try {
      const first = services.checkoutCommerceCart(db, {
        user_id: 1,
        items: [{ item_id: 1, quantity: 2 }],
      }, auth);
      expect(first.ok).toBe(true);
      const session = await services.createCommercePaymentSession(db, first.data.order_id, auth);
      expect(session.ok).toBe(true);

      db.query("UPDATE booking SET payment_deadline_at = ? WHERE id = ?").run(new Date(Date.now() - 60000).toISOString(), first.data.order_id);
      expect(services.expireCommerceOrders(db).data.expired).toBe(1);

      const second = services.checkoutCommerceCart(db, {
        user_id: 1,
        items: [{ item_id: 1, quantity: 2 }],
      }, auth);
      expect(second.ok).toBe(true);

      const late = await services.handleCommercePaymentWebhook(db, {
        event_id: "evt_late_authorized",
        reference: session.data.reference,
        status: "authorized",
        provider: "fake",
        occurred_at: new Date(Date.now() - 120000).toISOString(),
      });
      expect(late.ok).toBe(true);
      expect(late.data.status).toBe("cancelled");
      expect(late.data.idempotent).toBe(false);

      const booking = db.query<{ status: string }, []>("SELECT status FROM booking WHERE id = ?").get(first.data.order_id);
      const transaction = db.query<{ status: string }, []>("SELECT status FROM [transaction] WHERE id = ?").get(session.data.transaction_id);
      expect(booking?.status).toBe("cancelled");
      expect(transaction?.status).toBe("cancelled");
    } finally {
      db.close();
      delete process.env.PAYMENT_PROVIDER;
    }
  });

  test("late captured payment becomes refund-required then refunded when capacity has gone", async () => {
    const schema = await loadDuchyOperaSchema();
    const dir = writeGenerated(schema);
    const services = await import(`${pathToFileURL(join(dir, "services.ts")).href}?late-captured=${Date.now()}`);
    const dbPath = join(dir, "duchy-late-captured.sqlite");
    applyDuchySchema(schema, dbPath, { venueCapacity: 2 });
    const db = new Database(dbPath);
    const auth = { userId: null, scopes: ["*"] };
    process.env.PAYMENT_PROVIDER = "fake";

    try {
      const first = services.checkoutCommerceCart(db, {
        user_id: 1,
        items: [{ item_id: 1, quantity: 2 }],
      }, auth);
      expect(first.ok).toBe(true);
      const session = await services.createCommercePaymentSession(db, first.data.order_id, auth);
      expect(session.ok).toBe(true);

      db.query("UPDATE booking SET payment_deadline_at = ? WHERE id = ?").run(new Date(Date.now() - 60000).toISOString(), first.data.order_id);
      expect(services.expireCommerceOrders(db).data.expired).toBe(1);

      const second = services.checkoutCommerceCart(db, {
        user_id: 1,
        items: [{ item_id: 1, quantity: 2 }],
      }, auth);
      expect(second.ok).toBe(true);

      const captured = await services.handleCommercePaymentWebhook(db, {
        event_id: "evt_late_captured",
        reference: session.data.reference,
        status: "captured",
        provider: "fake",
        occurred_at: new Date(Date.now() - 120000).toISOString(),
      });
      expect(captured.ok).toBe(true);
      expect(captured.data.status).toBe("refund_required");

      const duplicate = await services.handleCommercePaymentWebhook(db, {
        event_id: "evt_late_captured",
        reference: session.data.reference,
        status: "captured",
        provider: "fake",
      });
      expect(duplicate.ok).toBe(true);
      expect(duplicate.data).toMatchObject({ status: "refund_required", idempotent: true });

      const refunded = await services.handleCommercePaymentWebhook(db, {
        event_id: "evt_late_refunded",
        reference: session.data.reference,
        status: "refunded",
        provider: "fake",
      });
      expect(refunded.ok).toBe(true);
      expect(refunded.data.status).toBe("refunded");

      const booking = db.query<{ status: string }, []>("SELECT status FROM booking WHERE id = ?").get(first.data.order_id);
      const transaction = db.query<{ status: string }, []>("SELECT status FROM [transaction] WHERE id = ?").get(session.data.transaction_id);
      expect(booking?.status).toBe("refunded");
      expect(transaction?.status).toBe("refunded");
    } finally {
      db.close();
      delete process.env.PAYMENT_PROVIDER;
    }
  });

  test("failed and cancelled local payment events cancel orders and tickets", async () => {
    const schema = await loadDuchyOperaSchema();
    const dir = writeGenerated(schema);
    const services = await import(`${pathToFileURL(join(dir, "services.ts")).href}?cancel-events=${Date.now()}`);
    const auth = { userId: null, scopes: ["*"] };

    for (const status of ["failed", "cancelled"] as const) {
      const dbPath = join(dir, `duchy-${status}.sqlite`);
      applyDuchySchema(schema, dbPath);
      const db = new Database(dbPath);
      try {
        const checkout = services.checkoutCommerceCart(db, {
          user_id: 1,
          items: [{ item_id: 1, quantity: 1, options: { ticket_type: "standard" } }],
        }, auth);
        expect(checkout.ok).toBe(true);
        const session = await services.createCommercePaymentSession(db, checkout.data.order_id, auth);
        expect(session.ok).toBe(true);
        const event = await services.handleCommercePaymentWebhook(db, {
          event_id: `evt_${status}`,
          reference: session.data.reference,
          status,
          provider: "local",
        });
        expect(event.ok).toBe(true);
        expect(event.data.status).toBe("cancelled");

        const booking = db.query<{ status: string }, []>("SELECT status FROM booking WHERE id = ?").get(checkout.data.order_id);
        const transaction = db.query<{ status: string }, []>("SELECT status FROM [transaction] WHERE id = ?").get(session.data.transaction_id);
        const ticket = db.query<{ status: string }, []>("SELECT status FROM ticket WHERE id IN (SELECT ticket_id FROM booking_ticket WHERE booking_id = ?)").get(checkout.data.order_id);
        expect(booking?.status).toBe("cancelled");
        expect(transaction?.status).toBe(status);
        expect(ticket?.status).toBe("cancelled");
      } finally {
        db.close();
      }
    }
  });

  test("non-local payment providers are explicit adapter work", async () => {
    const schema = await loadDuchyOperaSchema();
    const dir = writeGenerated(schema);
    const dbPath = join(dir, "duchy-provider-contract.sqlite");

    process.env.DB_PATH = dbPath;
    process.env.PORT = "0";
    process.env.AUTH_ENABLED = "false";
    process.env.PAYMENT_PROVIDER = "stripe";

    const { server } = await import(`${pathToFileURL(join(dir, "server.ts")).href}?provider-contract=${Date.now()}`);
    const base = `http://127.0.0.1:${server.port}`;
    seedDuchyOpera(dbPath);

    try {
      const checkout = await fetch(`${base}/commerce/checkout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          user_id: 1,
          client: "web",
          items: [{ item_id: 1, quantity: 1, options: { ticket_type: "standard" } }],
        }),
      });
      expect(checkout.status).toBe(201);
      const checkedOut = await checkout.json() as { order_id: number; amount_pence: number };

      const session = await fetch(`${base}/commerce/orders/${checkedOut.order_id}/payment-session`, { method: "POST" });
      expect(session.status).toBe(500);
      expect(await session.json()).toMatchObject({
        error: "unsupported PAYMENT_PROVIDER adapter stripe",
        code: "internal_error",
      });
    } finally {
      server.stop(true);
      clearEnv();
    }
  });

  test("commerce payment sessions support the fake provider for local development", async () => {
    const schema = await loadDuchyOperaSchema();
    const dir = writeGenerated(schema);
    const services = await import(pathToFileURL(join(dir, "services.ts")).href);
    const dbPath = join(dir, "duchy-fake-provider.sqlite");
    let db: Database | null = new Database(dbPath);
    const auth = { userId: null, scopes: ["*"] };
    process.env.PAYMENT_PROVIDER = "fake";

    try {
      for (const stmt of genSQL(schema.tables, schema.constraints).split(/;\s*\n/).filter(s => s.trim())) db.run(stmt);
      db.close();
      db = null;
      seedDuchyOpera(dbPath);
      db = new Database(dbPath);

      const checkout = services.checkoutCommerceCart(db, {
        user_id: 1,
        items: [{ item_id: 1, quantity: 1 }],
      }, auth);
      expect(checkout.ok).toBe(true);

      const intent = await services.createCommercePaymentSession(db, checkout.data.order_id, auth);
      expect(intent.ok).toBe(true);
      expect(intent.data.provider).toBe("fake");
      expect(intent.data.reference).toStartWith("fake_ps_");
      expect(intent.data.status).toBe("payment_pending");
      expect(intent.data.next_action?.type).toBe("redirect");
      expect(String(intent.data.next_action?.url)).toContain("provider=fake");

      const repeated = await services.createCommercePaymentSession(db, checkout.data.order_id, auth);
      expect(repeated.ok).toBe(true);
      expect(repeated.data).toMatchObject({
        provider: "fake",
        reference: intent.data.reference,
        client_secret: intent.data.client_secret,
      });
      expect(repeated.data.next_action).toEqual(intent.data.next_action);
    } finally {
      db?.close();
      delete process.env.PAYMENT_PROVIDER;
    }
  });
});
