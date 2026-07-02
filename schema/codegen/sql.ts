import assert from "assert/strict";
import type { Column, Constraints, Expr, Indexes, NormalizedConstraints, Tables } from "./types";
import { quoteReserved } from "./utils";
import { normalizeConstraints } from "./constraints";

export function sqlType(col: Column, availableTables?: string[]): string {
  const parts = [col.type.toUpperCase()];
  if (col.pk) parts.push("PRIMARY KEY");
  if (col.auto) parts.push("AUTOINCREMENT");
  if (col.required && !col.pk) parts.push("NOT NULL");
  if (col.unique) parts.push("UNIQUE");
  if (col.default !== null) parts.push(`DEFAULT ${col.default}`);
  if (col.references !== null) {
    // Validate that referenced table exists
    const match = col.references.match(/^(\w+)\(/);
    const targetTable = match?.[1];
    assert(
      !availableTables || !targetTable || availableTables.includes(targetTable),
      `Foreign key reference to non-existent table: ${col.references}\n` +
      `Available tables: ${availableTables?.join(", ")}\n` +
      `Either include the referenced table's module or remove the foreign key.`
    );
    // Quote table name in references if reserved
    const ref = col.references.replace(/^(\w+)/, (_, table) => quoteReserved(table));
    parts.push(`REFERENCES ${ref}`);
  }
  return parts.join(" ");
}

export function inferPerformanceIndexes(tables: Tables, explicitIndexes: Indexes = {}): Indexes {
  const inferred: Indexes = {};
  for (const [table, cols] of Object.entries(tables)) {
    const existing = explicitIndexes[table] || {};
    const names = new Set(Object.keys(existing));
    const signatures = new Set(Object.values(existing).map((index) => index.columns.join("\0")));
    const hasStatus = Boolean(cols.status);
    const temporalField = firstExistingField(cols, ["starts_at", "start_at", "scheduled_at", "date"]);
    const facetField = firstExistingField(cols, ["kind", "category"]);
    const foreignKeys = Object.entries(cols)
      .filter(([, col]) => col.references && !col.pk && !col.unique)
      .map(([field]) => field);

    if (hasStatus && foreignKeys.length === 0) {
      addInferredIndex(inferred, table, names, signatures, "by_status", ["status"]);
    }

    for (const field of foreignKeys) {
      const base = field.replace(/_id$/, "");
      if (temporalField) {
        addInferredIndex(inferred, table, names, signatures, `by_${base}_${temporalIndexSuffix(temporalField)}`, [field, temporalField]);
      } else if (facetField) {
        addInferredIndex(inferred, table, names, signatures, `by_${base}_${facetField}${hasStatus ? "_status" : ""}`, [field, facetField, ...(hasStatus ? ["status"] : [])]);
      } else if (hasStatus) {
        addInferredIndex(inferred, table, names, signatures, `by_${base}_status`, [field, "status"]);
      } else {
        addInferredIndex(inferred, table, names, signatures, `by_${base}`, [field]);
      }
    }
  }
  return inferred;
}

function firstExistingField(cols: Record<string, Column>, fields: string[]): string | null {
  return fields.find((field) => Boolean(cols[field])) || null;
}

function temporalIndexSuffix(field: string): string {
  if (field === "date") return "date";
  return "start";
}

function addInferredIndex(
  indexes: Indexes,
  table: string,
  names: Set<string>,
  signatures: Set<string>,
  name: string,
  columns: string[],
): void {
  const signature = columns.join("\0");
  if (names.has(name) || signatures.has(signature)) return;
  indexes[table] ||= {};
  indexes[table][name] = { columns, unique: false };
  names.add(name);
  signatures.add(signature);
}

function mergeIndexes(explicitIndexes: Indexes, inferredIndexes: Indexes): Indexes {
  const merged: Indexes = { ...explicitIndexes };
  for (const [table, indexes] of Object.entries(inferredIndexes)) {
    merged[table] = { ...(merged[table] || {}), ...indexes };
  }
  return merged;
}

function indexesForUniqueConstraints(tables: Tables, constraints: NormalizedConstraints = {}): Indexes {
  const indexes: Indexes = {};
  for (const [table, tableConstraints] of Object.entries(constraints)) {
    const cols = tables[table];
    assert(cols, `Constraint configured for non-existent table: ${table}`);
    for (const [name, constraint] of Object.entries(tableConstraints)) {
      if (constraint.kind !== "unique") continue;
      if (constraint.enforcement === "application") continue;
      assert(constraint.fields.length > 0, `Constraint ${name} on ${table} must include at least one field`);
      const columns = constraint.fields.map((field) => {
        assert(field.table === table, `Constraint ${name} on ${table} references non-local field: ${field.table}.${field.field}`);
        assert(cols[field.field], `Constraint ${name} on ${table} references non-existent column: ${field.field}`);
        return field.field;
      });
      indexes[table] ||= {};
      indexes[table][name] = { columns, unique: true, condition: constraint.condition };
    }
  }
  return indexes;
}

function dbCheckConstraintsForTable(table: string, cols: Record<string, Column>, constraints: NormalizedConstraints = {}): string[] {
  const checks: string[] = [];
  for (const [name, constraint] of Object.entries(constraints[table] || {})) {
    if (!databaseCheckConstraint(constraint)) continue;
    if (constraint.enforcement === "application") continue;
    const expression = sqlConstraintExpression(name, table, cols, constraint);
    const condition = constraint.condition ? compileSqlExpr(constraint.condition, cols) : "";
    const check = condition ? `((NOT ${condition}) OR ${expression})` : expression;
    checks.push(`    CONSTRAINT ${quoteReserved(`${table}_${name}`)} CHECK (${check})`);
  }
  return checks;
}

function databaseCheckConstraint(constraint: NormalizedConstraint): boolean {
  return constraint.kind === "check"
    || constraint.kind === "enum"
    || constraint.kind === "length"
    || constraint.kind === "range";
}

function sqlConstraintExpression(
  name: string,
  table: string,
  cols: Record<string, Column>,
  constraint: NormalizedConstraint,
): string {
  if (constraint.kind === "check") {
    assert(constraint.expression, `Constraint ${name} on ${table} requires an expression`);
    return compileSqlExpr(constraint.expression, cols);
  }

  assert(constraint.field, `Constraint ${name} on ${table} requires a field`);
  assert(constraint.field.table === table, `Constraint ${name} on ${table} references non-local field: ${constraint.field.table}.${constraint.field.field}`);
  assert(cols[constraint.field.field], `Constraint ${name} on ${table} references non-existent column: ${constraint.field.field}`);
  const field = quoteReserved(constraint.field.field);

  if (constraint.kind === "enum") {
    const values = constraint.values || [];
    assert(values.length > 0, `Constraint ${name} on ${table} requires at least one enum value`);
    return `(${field} IN (${values.map(sqlLiteral).join(", ")}))`;
  }
  if (constraint.kind === "length") {
    const parts: string[] = [];
    if (constraint.min !== null && constraint.min !== undefined) parts.push(`length(${field}) >= ${sqlLiteral(constraint.min)}`);
    if (constraint.max !== null && constraint.max !== undefined) parts.push(`length(${field}) <= ${sqlLiteral(constraint.max)}`);
    assert(parts.length > 0, `Constraint ${name} on ${table} requires min or max`);
    return `(${parts.join(" AND ")})`;
  }
  if (constraint.kind === "range") {
    const parts: string[] = [];
    if (constraint.min !== null && constraint.min !== undefined) parts.push(`${field} >= ${sqlLiteral(constraint.min)}`);
    if (constraint.max !== null && constraint.max !== undefined) parts.push(`${field} <= ${sqlLiteral(constraint.max)}`);
    assert(parts.length > 0, `Constraint ${name} on ${table} requires min or max`);
    return `(${parts.join(" AND ")})`;
  }

  throw new Error(`Unsupported SQL constraint kind: ${constraint.kind}`);
}

function compileSqlExpr(expr: Expr, cols: Record<string, Column>): string {
  switch (expr._t) {
    case "field": {
      const name = String(expr.name || "");
      assert(cols[name], `Constraint expression references non-existent column: ${name}`);
      return quoteReserved(name);
    }
    case "lit":
      return sqlLiteral(expr.value);
    case "bin": {
      const left = compileSqlExpr(expr.left as Expr, cols);
      const right = compileSqlExpr(expr.right as Expr, cols);
      const op = sqlBinaryOperator(String(expr.op || ""));
      return `(${left} ${op} ${right})`;
    }
    case "un": {
      const arg = compileSqlExpr(expr.arg as Expr, cols);
      if (expr.op === "isNull") return `(${arg} IS NULL)`;
      if (expr.op === "notNull") return `(${arg} IS NOT NULL)`;
      if (expr.op === "!") return `(NOT ${arg})`;
      throw new Error(`Unsupported SQL constraint unary operator: ${String(expr.op)}`);
    }
    default:
      throw new Error(`Unsupported SQL constraint expression type: ${String(expr._t)}`);
  }
}

function sqlBinaryOperator(op: string): string {
  if (op === "==") return "=";
  if (op === "!=") return "!=";
  if (["<", "<=", ">", ">=", "=", "+", "-", "*", "/", "AND", "OR"].includes(op)) return op;
  if (op === "&&") return "AND";
  if (op === "||") return "OR";
  throw new Error(`Unsupported SQL constraint binary operator: ${op}`);
}

function sqlLiteral(value: unknown): string {
  if (value === null || value === undefined) return "NULL";
  if (typeof value === "number") return Number.isFinite(value) ? String(value) : "NULL";
  if (typeof value === "boolean") return value ? "1" : "0";
  return `'${String(value).replace(/'/g, "''")}'`;
}

function genIndexSQL(tables: Tables, indexes: Indexes): string[] {
  const stmts: string[] = [];
  for (const [table, tableIndexes] of Object.entries(indexes)) {
    const cols = tables[table];
    assert(cols, `Index configured for non-existent table: ${table}`);
    for (const [name, index] of Object.entries(tableIndexes)) {
      assert(index.columns.length > 0, `Index ${name} on ${table} must include at least one column`);
      for (const column of index.columns) {
        assert(cols[column], `Index ${name} on ${table} references non-existent column: ${column}`);
      }
      const unique = index.unique ? "UNIQUE " : "";
      const tableName = quoteReserved(table);
      const indexName = `${table}_${name}`;
      const where = index.condition ? ` WHERE ${compileSqlExpr(index.condition, cols)}` : "";
      stmts.push(`CREATE ${unique}INDEX IF NOT EXISTS ${quoteReserved(indexName)} ON ${tableName} (${index.columns.join(", ")})${where};`);
    }
  }
  return stmts;
}

export function genSQL(tables: Tables, constraints: Constraints = {}): string {
  const normalizedConstraints = normalizeConstraints(constraints, tables);
  const constraintIndexes = indexesForUniqueConstraints(tables, normalizedConstraints);
  const allIndexes = mergeIndexes(constraintIndexes, inferPerformanceIndexes(tables, constraintIndexes));
  // Topological sort: tables with no FK deps first
  const tableNames = Object.keys(tables);
  const deps: Record<string, string[]> = {};
  for (const [table, cols] of Object.entries(tables)) {
    deps[table] = [];
    for (const c of Object.values(cols)) {
      if (c.references) {
        const match = c.references.match(/^(\w+)\(/);
        if (match && match[1] !== table && tableNames.includes(match[1])) {
          deps[table].push(match[1]);
        }
      }
    }
  }

  const sorted: string[] = [];
  const visited = new Set<string>();
  function visit(t: string) {
    if (visited.has(t) || !tables[t]) return;
    visited.add(t);
    for (const d of deps[t] || []) visit(d);
    sorted.push(t);
  }
  for (const t of tableNames) visit(t);

  const stmts: string[] = [];
  for (const table of sorted) {
    const cols = tables[table];
    if (!cols) continue;
    const defs: string[] = [];
    for (const [col, c] of Object.entries(cols)) {
      defs.push(`    ${col} ${sqlType(c, tableNames)}`);
    }
    defs.push(...dbCheckConstraintsForTable(table, cols, normalizedConstraints));
    const tableName = quoteReserved(table);
    stmts.push(`CREATE TABLE IF NOT EXISTS ${tableName} (\n${defs.join(",\n")}\n);`);
  }
  stmts.push(...genIndexSQL(tables, allIndexes));
  return stmts.join("\n\n") + "\n";
}
