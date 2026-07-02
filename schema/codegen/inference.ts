import type {
  Column,
  CommerceOption,
  FieldRef,
  InferredCommerceConfig,
  ProfileExtension,
  Schema,
  Tables,
} from "./types";
import { normalizeConstraints } from "./constraints";

export interface InferenceDiagnostic {
  path: string;
  message: string;
}

export interface CommerceInference {
  config: InferredCommerceConfig | null;
  diagnostics: InferenceDiagnostic[];
}

export function parseReference(reference: string | null | undefined): { table: string; field: string } | null {
  if (!reference) return null;
  const fk = reference.match(/^([A-Za-z_][A-Za-z0-9_]*)\(([A-Za-z_][A-Za-z0-9_]*)\)$/);
  if (fk) return { table: fk[1], field: fk[2] };
  const dotted = reference.match(/^([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)$/);
  if (dotted) return { table: dotted[1], field: dotted[2] };
  return null;
}

export function refFor(tables: Tables, table: string, field: string): FieldRef {
  return {
    table,
    field,
    references: tables[table]?.[field]?.references ?? null,
  };
}

export function inferProfileExtensions(schema: Schema): ProfileExtension[] {
  const constraints = normalizeConstraints(schema.constraints || {}, schema.tables);
  const result: ProfileExtension[] = [];

  for (const [entity, columns] of Object.entries(schema.tables)) {
    const fkFields = Object.entries(columns)
      .map(([field, column]) => ({ field, column, target: parseReference(column.references) }))
      .filter((entry): entry is { field: string; column: Column; target: { table: string; field: string } } => Boolean(entry.target));
    if (fkFields.length !== 1) continue;

    const fk = fkFields[0];
    if (!fk.column.required || !fieldIsUnique(schema, constraints, entity, fk.field)) continue;
    result.push({
      entity,
      parentEntity: fk.target.table,
      field: refFor(schema.tables, entity, fk.field),
    });
  }

  return result.sort((a, b) => a.parentEntity.localeCompare(b.parentEntity) || a.entity.localeCompare(b.entity));
}

function fieldIsUnique(
  schema: Schema,
  constraints: ReturnType<typeof normalizeConstraints>,
  entity: string,
  field: string,
): boolean {
  if (schema.tables[entity]?.[field]?.unique) return true;
  return Object.values(constraints[entity] || {}).some((constraint) =>
    constraint.kind === "unique" &&
    constraint.fields.length === 1 &&
    constraint.fields[0].table === entity &&
    constraint.fields[0].field === field
  );
}

export function inferCommerce(schema: Schema): CommerceInference {
  if (!schema.commerce?.enabled) return { config: null, diagnostics: [] };

  const diagnostics: InferenceDiagnostic[] = [];
  const order = chooseOne("commerce.inference.order", diagnostics, Object.keys(schema.tables).filter((entity) => isOrder(schema, entity)));
  const transaction = chooseOne("commerce.inference.transaction", diagnostics, Object.keys(schema.tables).filter((entity) => isTransaction(schema, entity)));
  const lineItem = chooseOne("commerce.inference.lineItem", diagnostics, Object.keys(schema.tables).filter((entity) => isLineItem(schema, entity)));

  if (!order || !transaction || !lineItem) return { config: null, diagnostics };

  const catalogItem = chooseOne(
    "commerce.inference.lineItem.catalogItem",
    diagnostics,
    foreignKeys(schema, lineItem)
      .filter((fk) => !["user", order, transaction].includes(fk.target.table))
      .filter((fk) => moneyField(schema, fk.target.table, ["price_pence", "amount_pence"])),
  );
  if (!catalogItem) return { config: null, diagnostics };

  const catalog = catalogItem.target.table;
  const orderLine = chooseOne("commerce.inference.orderLine", diagnostics, Object.keys(schema.tables).filter((entity) => hasFkTo(schema, entity, order) && hasFkTo(schema, entity, lineItem)));
  const transactionLine = chooseOne("commerce.inference.transactionLine", diagnostics, Object.keys(schema.tables).filter((entity) => hasFkTo(schema, entity, transaction) && hasFkTo(schema, entity, lineItem)));
  if (!orderLine || !transactionLine) return { config: null, diagnostics };

  const catalogPrice = moneyField(schema, catalog, ["price_pence"]);
  const orderUser = fkTo(schema, order, "user");
  const orderStatus = enumField(schema, order, ["checkout_pending", "paid", "expired", "cancelled"]);
  const orderAmount = moneyField(schema, order, ["amount_pence"]);
  const orderCurrency = fieldNamed(schema, order, ["currency"]);
  const orderExpiresAt = fieldNamed(schema, order, ["expires_at"]);
  const orderPaymentReference = fieldNamed(schema, order, ["payment_reference"]);
  const lineStatus = enumField(schema, lineItem, ["reserved", "cancelled"]);
  const linePrice = moneyField(schema, lineItem, ["price_pence"]);
  const txUser = fkTo(schema, transaction, "user");
  const txAmount = moneyField(schema, transaction, ["amount_pence"]);
  const txStatus = enumField(schema, transaction, ["pending", "completed", "failed"]);
  const txReference = fieldNamed(schema, transaction, ["reference", "payment_reference"]);

  requiredRef(diagnostics, "commerce.inference.catalog.price", catalogPrice);
  requiredRef(diagnostics, "commerce.inference.order.user", orderUser);
  requiredRef(diagnostics, "commerce.inference.order.status", orderStatus);
  requiredRef(diagnostics, "commerce.inference.order.amount", orderAmount);
  requiredRef(diagnostics, "commerce.inference.order.currency", orderCurrency);
  requiredRef(diagnostics, "commerce.inference.order.expiresAt", orderExpiresAt);
  requiredRef(diagnostics, "commerce.inference.order.paymentReference", orderPaymentReference);
  requiredRef(diagnostics, "commerce.inference.lineItem.status", lineStatus);
  requiredRef(diagnostics, "commerce.inference.lineItem.price", linePrice);
  requiredRef(diagnostics, "commerce.inference.transaction.user", txUser);
  requiredRef(diagnostics, "commerce.inference.transaction.amount", txAmount);
  requiredRef(diagnostics, "commerce.inference.transaction.status", txStatus);
  requiredRef(diagnostics, "commerce.inference.transaction.reference", txReference);
  if (diagnostics.length > 0) return { config: null, diagnostics };

  const lineEnums = enumValues(schema.tables[lineItem][lineStatus!.field]);
  const txEnums = enumValues(schema.tables[transaction][txStatus!.field]);

  return {
    diagnostics,
    config: {
      enabled: true,
      catalog: {
        entity: catalog,
        label: labelField(schema, catalog),
        description: fieldNamed(schema, catalog, ["description", "summary"]),
        price: catalogPrice,
        groupBy: labelField(schema, catalog) ? [labelField(schema, catalog)!] : [],
        variantFields: variantFields(schema, catalog, labelField(schema, catalog)),
        availability: availability(schema, catalog),
      },
      order: {
        entity: order,
        user: orderUser,
        status: orderStatus,
        amount: orderAmount,
        currency: orderCurrency,
        expiresAt: orderExpiresAt,
        paymentReference: orderPaymentReference,
        client: fieldNamed(schema, order, ["client"]),
        pendingStatus: "checkout_pending",
        paidStatus: "paid",
        expiredStatus: "expired",
        cancelledStatus: "cancelled",
      },
      lineItem: {
        entity: lineItem,
        catalogItem: refFor(schema.tables, lineItem, catalogItem.field),
        user: fkTo(schema, lineItem, "user"),
        price: linePrice,
        status: lineStatus,
        quantity: fieldNamed(schema, lineItem, ["quantity"]),
        reservedStatus: "reserved",
        fulfilledStatus: lineEnums.includes("confirmed") ? "confirmed" : "fulfilled",
        cancelledStatus: "cancelled",
        options: commerceOptions(schema, lineItem, new Set([catalogItem.field, fkTo(schema, lineItem, "user")?.field, linePrice!.field, lineStatus!.field].filter(Boolean) as string[])),
      },
      orderLine: {
        entity: orderLine,
        order: fkTo(schema, orderLine, order),
        lineItem: fkTo(schema, orderLine, lineItem),
      },
      transaction: {
        entity: transaction,
        user: txUser,
        amount: txAmount,
        type: fieldNamed(schema, transaction, ["type"]),
        status: txStatus,
        reference: txReference,
        client: fieldNamed(schema, transaction, ["client"]),
        purchaseType: enumValues(schema.tables[transaction][fieldNamed(schema, transaction, ["type"])?.field || ""] || {}).includes("purchase") ? "purchase" : "purchase",
        pendingStatus: "pending",
        completedStatus: txEnums.includes("completed") ? "completed" : "paid",
        failedStatus: "failed",
      },
      transactionLine: {
        entity: transactionLine,
        transaction: fkTo(schema, transactionLine, transaction),
        lineItem: fkTo(schema, transactionLine, lineItem),
      },
      checkout: {
        currency: schema.commerce.checkout?.currency || "GBP",
        expiryMinutes: schema.commerce.checkout?.expiryMinutes ?? 15,
        maxQuantity: schema.commerce.checkout?.maxQuantity ?? 20,
        maxLines: schema.commerce.checkout?.maxLines ?? 50,
      },
    },
  };
}

export function inferCommerceConfig(schema: Schema): InferredCommerceConfig | null {
  const inference = inferCommerce(schema);
  if (inference.diagnostics.length > 0) {
    throw new Error(inference.diagnostics.map((diagnostic) => `${diagnostic.path}: ${diagnostic.message}`).join("\n"));
  }
  return inference.config;
}

function chooseOne<T>(path: string, diagnostics: InferenceDiagnostic[], candidates: T[]): T | null {
  if (candidates.length === 1) return candidates[0];
  diagnostics.push({
    path,
    message: candidates.length === 0
      ? "could not infer a matching role"
      : `is ambiguous: ${candidates.map(String).join(", ")}`,
  });
  return null;
}

function requiredRef(diagnostics: InferenceDiagnostic[], path: string, ref: FieldRef | null): void {
  if (!ref) diagnostics.push({ path, message: "could not infer a required field" });
}

function isOrder(schema: Schema, entity: string): boolean {
  return Boolean(
    fkTo(schema, entity, "user") &&
    enumField(schema, entity, ["checkout_pending", "paid", "expired", "cancelled"]) &&
    moneyField(schema, entity, ["amount_pence"]) &&
    fieldNamed(schema, entity, ["currency"]) &&
    fieldNamed(schema, entity, ["expires_at"]) &&
    fieldNamed(schema, entity, ["payment_reference"]),
  );
}

function isTransaction(schema: Schema, entity: string): boolean {
  return Boolean(
    fkTo(schema, entity, "user") &&
    enumField(schema, entity, ["pending", "completed", "failed"]) &&
    moneyField(schema, entity, ["amount_pence"]) &&
    fieldNamed(schema, entity, ["reference", "payment_reference"]),
  );
}

function isLineItem(schema: Schema, entity: string): boolean {
  return Boolean(
    enumField(schema, entity, ["reserved", "cancelled"]) &&
    moneyField(schema, entity, ["price_pence"]) &&
    foreignKeys(schema, entity).some((fk) => fk.target.table !== "user"),
  );
}

function foreignKeys(schema: Schema, entity: string): Array<{ field: string; column: Column; target: { table: string; field: string } }> {
  return Object.entries(schema.tables[entity] || {})
    .map(([field, column]) => ({ field, column, target: parseReference(column.references) }))
    .filter((entry): entry is { field: string; column: Column; target: { table: string; field: string } } => Boolean(entry.target));
}

function hasFkTo(schema: Schema, entity: string, target: string): boolean {
  return foreignKeys(schema, entity).some((fk) => fk.target.table === target);
}

function fkTo(schema: Schema, entity: string, target: string): FieldRef | null {
  const fk = foreignKeys(schema, entity).find((candidate) => candidate.target.table === target);
  return fk ? refFor(schema.tables, entity, fk.field) : null;
}

function fieldNamed(schema: Schema, entity: string, names: string[]): FieldRef | null {
  for (const name of names) {
    if (schema.tables[entity]?.[name]) return refFor(schema.tables, entity, name);
  }
  return null;
}

function moneyField(schema: Schema, entity: string, names: string[]): FieldRef | null {
  for (const name of names) {
    const col = schema.tables[entity]?.[name];
    if (col && (name.endsWith("_pence") || col.metadata?.format === "money")) return refFor(schema.tables, entity, name);
  }
  const found = Object.entries(schema.tables[entity] || {}).find(([field, col]) => field.endsWith("_pence") || col.metadata?.format === "money");
  return found ? refFor(schema.tables, entity, found[0]) : null;
}

function enumField(schema: Schema, entity: string, required: string[]): FieldRef | null {
  for (const [field, col] of Object.entries(schema.tables[entity] || {})) {
    const values = enumValues(col);
    if (required.every((value) => values.includes(value))) return refFor(schema.tables, entity, field);
  }
  return null;
}

function enumValues(col: Column | undefined): string[] {
  return col?.constraints?.enum || [];
}

function labelField(schema: Schema, entity: string): FieldRef | null {
  const direct = fieldNamed(schema, entity, ["name", "title"]);
  if (direct) return direct;
  const fk = foreignKeys(schema, entity).find((candidate) => candidate.target.table !== "venue" && candidate.target.table !== "user")
    || foreignKeys(schema, entity)[0];
  return fk ? refFor(schema.tables, entity, fk.field) : null;
}

function variantFields(schema: Schema, entity: string, label: FieldRef | null): FieldRef[] {
  const labelKey = label ? `${label.table}.${label.field}` : "";
  return Object.entries(schema.tables[entity] || {})
    .filter(([field, col]) => {
      if (`${entity}.${field}` === labelKey) return false;
      if (field === "id" || field === "created_at" || field === "updated_at") return false;
      if (parseReference(col.references)) return true;
      const format = col.metadata?.format || "";
      return format === "date" || format === "time" || format === "date-time" || field.endsWith("_at");
    })
    .map(([field]) => refFor(schema.tables, entity, field));
}

function availability(schema: Schema, entity: string): { field: FieldRef | null; available: string } {
  const status = fieldNamed(schema, entity, ["status"]);
  const values = status ? enumValues(schema.tables[entity][status.field]) : [];
  for (const value of ["scheduled", "available", "open", "active"]) {
    if (values.includes(value)) return { field: status, available: value };
  }
  return { field: status, available: values[0] || "available" };
}

function commerceOptions(schema: Schema, entity: string, excluded: Set<string>): Record<string, CommerceOption> {
  const options: Record<string, CommerceOption> = {};
  for (const [field, col] of Object.entries(schema.tables[entity] || {})) {
    if (excluded.has(field)) continue;
    if (field === "id" || field === "created_at" || field === "updated_at" || parseReference(col.references)) continue;
    options[field] = {
      field: refFor(schema.tables, entity, field),
      type: col.type,
      label: col.metadata?.label || titleCase(field),
      default: cleanDefault(col.default),
      choices: enumValues(col),
      required: Boolean(col.required && col.default === null),
      min: col.constraints?.range?.min ?? null,
      max: col.constraints?.range?.max ?? null,
    };
  }
  return options;
}

function cleanDefault(value: string | null): string | null {
  if (value === null || value === undefined) return null;
  const quoted = value.match(/^'(.*)'$/);
  return quoted ? quoted[1] : value;
}

function titleCase(value: string): string {
  return value
    .replace(/_/g, " ")
    .replace(/\b\w/g, (char) => char.toUpperCase())
    .replace(/\bId\b/g, "ID");
}
