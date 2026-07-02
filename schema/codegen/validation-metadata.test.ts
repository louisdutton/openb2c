import { describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { genOpenAPI } from "./openapi";
import { genServices } from "./services";
import { genSQL } from "./sql";
import { genTypes } from "./typescript";
import { validateSchema } from "./validation";
import type { Column, Operation, Schema } from "./types";

const baseColumn: Column = {
  type: "text",
  pk: false,
  auto: false,
  required: false,
  unique: false,
  default: null,
  references: null,
};

function col(overrides: Partial<Column>): Column {
  return { ...baseColumn, ...overrides };
}

function op(overrides: Partial<Operation> = {}): Operation {
  return { guard: null, relationships: [], public: true, scope: null, policy: {}, workflow: {}, set: {}, cascade: [], effects: [], ...overrides };
}

function validationSchema(): Schema {
  return {
    organization: { name: "Validation Test", description: "Validation metadata test app", logo: null },
    tables: {
      ticket: {
        id: col({ type: "integer", pk: true, auto: true }),
        ticket_type: col({
          default: "'standard'",
          constraints: { enum: ["standard", "vip"] },
        }),
        price_pence: col({
          type: "integer",
          required: true,
          constraints: { range: { min: 0, max: 100000 } },
        }),
      },
    },
    constraints: {
      ticket: {
        unique: {
          unique_ticket_type: {
            fields: [{ table: "ticket", field: "ticket_type", references: null }],
          },
        },
        check: {
          vip_price_minimum: {
            fields: [
              { table: "ticket", field: "ticket_type", references: null },
              { table: "ticket", field: "price_pence", references: null },
            ],
            expression: {
              _t: "bin",
              op: "||",
              left: {
                _t: "bin",
                op: "!=",
                left: { _t: "field", name: "ticket_type" },
                right: { _t: "lit", value: "vip" },
              },
              right: {
                _t: "bin",
                op: ">=",
                left: { _t: "field", name: "price_pence" },
                right: { _t: "lit", value: 2500 },
              },
            },
          },
        },
      },
    },
    operations: {
      ticket: {
        create: op(),
        update: op(),
      },
    },
  };
}

describe("validation metadata generation", () => {
  test("threads field validation and logical constraint metadata into OpenAPI", () => {
    const openapi = JSON.parse(genOpenAPI(validationSchema()));

    expect(openapi.components.schemas.TicketInput.properties.ticket_type.enum).toEqual(["standard", "vip"]);
    expect(openapi.components.schemas.TicketInput.properties.price_pence.minimum).toBe(0);
    expect(openapi.components.schemas.TicketInput.properties.price_pence.maximum).toBe(100000);
    expect(openapi["x-openb2c-validation"].constraints.ticket.unique_ticket_type).toMatchObject({
      kind: "unique",
      fields: [{ table: "ticket", field: "ticket_type", references: null }],
      message: "Ticket type must be unique.",
      enforcement: "auto",
    });
    expect(openapi["x-openb2c-validation"].constraints.ticket.vip_price_minimum).toMatchObject({
      kind: "check",
      fields: [
        { table: "ticket", field: "ticket_type", references: null },
        { table: "ticket", field: "price_pence", references: null },
      ],
      message: "VIP price minimum constraint failed.",
      enforcement: "auto",
    });
  });

  test("generated services enforce logical check constraints", async () => {
    const schema = validationSchema();
    const dir = mkdtempSync(join(tmpdir(), "openb2c-logical-constraints-"));
    writeFileSync(join(dir, "types.ts"), genTypes(schema.tables, schema.operations));
    writeFileSync(join(dir, "services.ts"), genServices(schema));

    const services = await import(`${pathToFileURL(join(dir, "services.ts")).href}?${Date.now()}`);
    const db = new Database(":memory:");
    db.exec(genSQL(schema.tables, schema.constraints));

    try {
      expect(services.createTicket(db, { ticket_type: "vip", price_pence: 2000 }).error).toBe("VIP price minimum constraint failed.");
      expect(services.createTicket(db, { ticket_type: "vip", price_pence: 2500 }).ok).toBe(true);
    } finally {
      db.close();
    }
  });

  test("validates malformed logical constraints", () => {
    const schema = validationSchema();
    schema.constraints!.ticket.check!.vip_price_minimum.fields = [
      { table: "ticket", field: "ticket_type", references: null },
    ];
    schema.constraints!.ticket.check!.vip_price_minimum.expression = { _t: "field", name: "missing" };
    schema.constraints!.ticket.check!.related = {
      fields: [
        { table: "ticket", field: "ticket_type", references: null },
        { table: "ticket", field: "price_pence", references: null },
      ],
      expression: { _t: "rel", entity: "user", field: "email" },
    } as any;

    expect(validateSchema(schema)).toEqual(expect.arrayContaining([
      expect.objectContaining({ path: "constraints.ticket.check.vip_price_minimum.expression.name" }),
      expect.objectContaining({ path: "constraints.ticket.check.vip_price_minimum.fields" }),
      expect.objectContaining({ path: "constraints.ticket.check.related.expression" }),
    ]));
  });
});
