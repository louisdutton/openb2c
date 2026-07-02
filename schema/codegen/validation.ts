import type { AssignmentSet, Cascade, Column, ConstraintEnforcement, Effect, Expr, FieldAssignment, FieldRef, Operation, Schema, SeedValue, TableRef, Tables } from "./types";
import { normalizeConstraints } from "./constraints";
import { inferCommerce } from "./inference";

export interface SchemaDiagnostic {
  path: string;
  message: string;
}

export class SchemaValidationError extends Error {
  diagnostics: SchemaDiagnostic[];

  constructor(diagnostics: SchemaDiagnostic[]) {
    super(formatSchemaDiagnostics(diagnostics));
    this.name = "SchemaValidationError";
    this.diagnostics = diagnostics;
  }
}

export function formatSchemaDiagnostics(diagnostics: SchemaDiagnostic[]): string {
  return [
    "Schema validation failed:",
    ...diagnostics.map(diagnostic => `- ${diagnostic.path}: ${diagnostic.message}`),
  ].join("\n");
}

export function assertValidSchema(schema: Schema): void {
  const diagnostics = validateSchema(schema);
  if (diagnostics.length > 0) {
    throw new SchemaValidationError(diagnostics);
  }
}

export function validateSchema(schema: Schema): SchemaDiagnostic[] {
  const diagnostics: SchemaDiagnostic[] = [];
  const tables = schema.tables || {};
  if ((schema as any).workflows !== undefined) add(diagnostics, "workflows", "has been replaced by presentation.groups");
  if ((schema as any).ecommerce !== undefined) add(diagnostics, "ecommerce", "has been replaced by commerce");
  const normalized = {
    ...schema,
    tables,
    derived: schema.derived || {},
    audit: schema.audit || { entities: {} },
    seed: schema.seed || { reference: {}, fixtures: {}, applyFixturesByDefault: false },
    integrations: schema.integrations,
    effectContracts: schema.effectContracts || { events: {}, notifications: { templates: {}, recipients: {} }, calls: {} },
    operations: schema.operations || {},
    constraints: schema.constraints || {},
    presentation: schema.presentation || { groups: {} },
    commerce: schema.commerce || { enabled: false, checkout: { currency: "GBP", expiryMinutes: 15, paymentDeadlineMinutes: 30, allowAsyncPayments: false, maxQuantity: 20, maxLines: 50 } },
  } as Schema;

  if (!schema.tables) add(diagnostics, "tables", "is required");
  if (!schema.operations) add(diagnostics, "operations", "is required");

  validateColumns(normalized, diagnostics);
  validateDerivedFields(normalized, diagnostics);
  validateAudit(normalized, diagnostics);
  validateSeed(normalized, diagnostics);
  validateIntegrations(normalized, diagnostics);
  validateEffectContracts(normalized, diagnostics);
  validateConstraints(normalized, diagnostics);
  validateRelationships(normalized, diagnostics);
  validatePresentation(normalized, diagnostics);
  validateOperations(normalized, diagnostics);
  validateCommerce(normalized, diagnostics);

  return diagnostics;
}

function add(diagnostics: SchemaDiagnostic[], path: string, message: string): void {
  diagnostics.push({ path, message });
}

function tableExists(tables: Tables, table: string): boolean {
  return Boolean(tables[table]);
}

function columnExists(tables: Tables, table: string, field: string): boolean {
  return Boolean(tables[table]?.[field]);
}

function parseReference(reference: string): { table: string; field: string } | null {
  const fk = reference.match(/^([A-Za-z_][A-Za-z0-9_]*)\(([A-Za-z_][A-Za-z0-9_]*)\)$/);
  if (fk) return { table: fk[1], field: fk[2] };
  const dotted = reference.match(/^([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)$/);
  if (dotted) return { table: dotted[1], field: dotted[2] };
  return null;
}

function validateReference(tables: Tables, reference: string, path: string, diagnostics: SchemaDiagnostic[]): void {
  const parsed = parseReference(reference);
  if (!parsed) {
    add(diagnostics, path, `must use table(field) or table.field syntax, got ${JSON.stringify(reference)}`);
    return;
  }
  if (!tableExists(tables, parsed.table)) {
    add(diagnostics, path, `references unknown table ${JSON.stringify(parsed.table)}`);
    return;
  }
  if (!columnExists(tables, parsed.table, parsed.field)) {
    add(diagnostics, path, `references unknown field ${parsed.table}.${parsed.field}`);
  }
}

function validateFieldRef(schema: Schema, ref: FieldRef | null | undefined, path: string, diagnostics: SchemaDiagnostic[], expectedTable?: string): void {
  if (!ref) {
    add(diagnostics, path, "is required");
    return;
  }
  if (expectedTable && ref.table !== expectedTable) {
    add(diagnostics, `${path}.table`, `must be ${JSON.stringify(expectedTable)}, got ${JSON.stringify(ref.table)}`);
  }
  if (!tableExists(schema.tables, ref.table)) {
    add(diagnostics, `${path}.table`, `references unknown table ${JSON.stringify(ref.table)}`);
    return;
  }
  if (!columnExists(schema.tables, ref.table, ref.field)) {
    add(diagnostics, `${path}.field`, `references unknown field ${ref.table}.${ref.field}`);
  }
  if (ref.references) validateReference(schema.tables, ref.references, `${path}.references`, diagnostics);
}

function validateOptionalFieldRef(schema: Schema, ref: FieldRef | null | undefined, path: string, diagnostics: SchemaDiagnostic[], expectedTable?: string): void {
  if (ref) validateFieldRef(schema, ref, path, diagnostics, expectedTable);
}

function validateColumns(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  const tables = schema.tables;
  for (const [table, cols] of Object.entries(tables)) {
    for (const [field, col] of Object.entries(cols)) {
      if (col.auto && !col.pk) {
        add(diagnostics, `tables.${table}.${field}.auto`, "auto columns must also be primary keys");
      }
      if (col.references) {
        validateReference(tables, col.references, `tables.${table}.${field}.references`, diagnostics);
      }
      validateColumnMetadata(table, field, col, diagnostics);
      validateColumnConstraints(table, field, col, diagnostics);
      validateColumnRelationship(schema, table, field, col, diagnostics);
    }
  }
}

function validateDerivedFields(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  for (const [entity, fields] of Object.entries(schema.derived || {})) {
    if (!tableExists(schema.tables, entity)) {
      add(diagnostics, `derived.${entity}`, `references unknown entity ${JSON.stringify(entity)}`);
      continue;
    }
    for (const [name, field] of Object.entries(fields)) {
      const path = `derived.${entity}.${name}`;
      if (columnExists(schema.tables, entity, name)) {
        add(diagnostics, path, `conflicts with stored field ${entity}.${name}`);
      }
      for (const [index, dependency] of (field.dependencies || []).entries()) {
        validateFieldRef(schema, dependency, `${path}.dependencies.${index}`, diagnostics, entity);
      }
      if (field.template && field.expression) {
        add(diagnostics, path, "must use either template or expression, not both");
      }
      if (!field.template && !field.expression) {
        add(diagnostics, path, "requires template or expression");
      }
      const dependencyFields = new Set((field.dependencies || []).filter(dependency => dependency.table === entity).map(dependency => dependency.field));
      for (const placeholder of templateFields(field.template)) {
        if (!columnExists(schema.tables, entity, placeholder)) {
          add(diagnostics, `${path}.template`, `references unknown field ${entity}.${placeholder}`);
        } else if (!dependencyFields.has(placeholder)) {
          add(diagnostics, `${path}.dependencies`, `must include template field ${entity}.${placeholder}`);
        }
      }
      if (field.expression) {
        validateConstraintExpr(schema, entity, field.expression, `${path}.expression`, diagnostics);
        for (const expressionField of expressionFields(field.expression)) {
          if (!dependencyFields.has(expressionField)) {
            add(diagnostics, `${path}.dependencies`, `must include expression field ${entity}.${expressionField}`);
          }
        }
      }
    }
  }
}

function templateFields(template: string | null | undefined): string[] {
  if (!template) return [];
  return [...template.matchAll(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g)].map(match => match[1]);
}

function validateColumnMetadata(table: string, field: string, col: Column, diagnostics: SchemaDiagnostic[]): void {
  const metadata = col.metadata;
  if (!metadata) return;
  if (metadata.privacy && !["public", "internal", "sensitive", "secret"].includes(metadata.privacy)) {
    add(diagnostics, `tables.${table}.${field}.metadata.privacy`, "must be one of public, internal, sensitive, or secret");
  }
}

function validateColumnConstraints(table: string, field: string, col: Column, diagnostics: SchemaDiagnostic[]): void {
  const constraints = col.constraints;
  if (!constraints) return;
  const path = `tables.${table}.${field}.constraints`;
  validateBounds(`${path}.length`, constraints.length?.min, constraints.length?.max, "min", "max", diagnostics);
  validateBounds(`${path}.range`, constraints.range?.min, constraints.range?.max, "min", "max", diagnostics);
  if (constraints.pattern) {
    try {
      new RegExp(constraints.pattern);
    } catch {
      add(diagnostics, `${path}.pattern`, "must be a valid JavaScript regular expression");
    }
  }
  if (numericColumn(col) && constraints.enum?.length) {
    for (const value of constraints.enum) {
      if (!Number.isFinite(Number(value))) {
        add(diagnostics, `${path}.enum`, `contains non-numeric value ${JSON.stringify(value)} for numeric field`);
      }
    }
  }
}

function validateColumnRelationship(schema: Schema, table: string, field: string, col: Column, diagnostics: SchemaDiagnostic[]): void {
  const relationship = col.relationship;
  if (!relationship) return;
  if (!col.references) {
    add(diagnostics, `tables.${table}.${field}.relationship`, "requires a foreign-key references value");
    return;
  }

  const parsed = parseReference(col.references);
  if (!parsed) return;
  if (relationship.cardinality && !["one", "many"].includes(relationship.cardinality)) {
    add(diagnostics, `tables.${table}.${field}.relationship.cardinality`, "must be one of one or many");
  }
  for (const key of Object.keys(relationship as Record<string, unknown>)) {
    if (!["label", "description", "cardinality"].includes(key)) {
      add(diagnostics, `tables.${table}.${field}.relationship.${key}`, "relationship metadata can only override label, description, or cardinality");
    }
  }
}

function validateRelationships(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  for (const [entity, relationships] of Object.entries(schema.relationships || {})) {
    if (!tableExists(schema.tables, entity)) {
      add(diagnostics, `relationships.${entity}`, `references unknown entity ${JSON.stringify(entity)}`);
      continue;
    }
    for (const [name, relationship] of Object.entries(relationships)) {
      validateFieldRef(schema, relationship.field, `relationships.${entity}.${name}.field`, diagnostics, entity);
    }
  }
}

function validateConstraints(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  const allowedKinds = ["unique", "check"];
  for (const [entity, group] of Object.entries(schema.constraints || {})) {
    if (!tableExists(schema.tables, entity)) {
      add(diagnostics, `constraints.${entity}`, `references unknown entity ${JSON.stringify(entity)}`);
      continue;
    }
    for (const kind of Object.keys(group)) {
      if (!allowedKinds.includes(kind)) {
        add(diagnostics, `constraints.${entity}.${kind}`, "must be one of unique or check");
      }
    }
    const names = new Set<string>();
    for (const [name, constraint] of Object.entries(group.unique || {})) {
      const path = `constraints.${entity}.unique.${name}`;
      if (names.has(name)) add(diagnostics, path, "duplicates another constraint name on this entity");
      names.add(name);
      validateEnforcement(constraint.enforcement, path, diagnostics);
      if (!constraint.fields || constraint.fields.length === 0) add(diagnostics, `${path}.fields`, "must include at least one field");
      for (const [index, field] of (constraint.fields || []).entries()) {
        validateFieldRef(schema, field, `${path}.fields.${index}`, diagnostics, entity);
      }
      if (constraint.condition) {
        validateConstraintExpr(schema, entity, constraint.condition, `${path}.condition`, diagnostics);
      }
    }
    for (const [name, constraint] of Object.entries(group.check || {})) {
      const path = `constraints.${entity}.check.${name}`;
      if (names.has(name)) add(diagnostics, path, "duplicates another constraint name on this entity");
      names.add(name);
      validateEnforcement(constraint.enforcement, path, diagnostics);
      if (!constraint.fields || constraint.fields.length === 0) add(diagnostics, `${path}.fields`, "must include at least one field");
      for (const [index, field] of (constraint.fields || []).entries()) {
        validateFieldRef(schema, field, `${path}.fields.${index}`, diagnostics, entity);
      }
      validateConstraintExpr(schema, entity, constraint.expression, `${path}.expression`, diagnostics);
      const declaredFields = new Set((constraint.fields || []).filter(field => field.table === entity).map(field => field.field));
      for (const field of expressionFields(constraint.expression)) {
        if (!declaredFields.has(field)) {
          add(diagnostics, `${path}.fields`, `must include expression field ${entity}.${field}`);
        }
      }
      if (constraint.condition) {
        validateConstraintExpr(schema, entity, constraint.condition, `${path}.condition`, diagnostics);
      }
    }
  }
}

function validateEnforcement(enforcement: ConstraintEnforcement | undefined, path: string, diagnostics: SchemaDiagnostic[]): void {
  if (enforcement && !["auto", "database", "application", "both"].includes(enforcement)) {
    add(diagnostics, `${path}.enforcement`, "must be one of auto, database, application, or both");
  }
}

function validateBounds(
  path: string,
  min: number | null | undefined,
  max: number | null | undefined,
  minName: string,
  maxName: string,
  diagnostics: SchemaDiagnostic[],
): void {
  if (min === null || min === undefined) {
    if (max === null || max === undefined) return;
  } else if (min < 0) {
    add(diagnostics, `${path}.${minName}`, "must be greater than or equal to 0");
  }

  if (max !== null && max !== undefined && max < 0) {
    add(diagnostics, `${path}.${maxName}`, "must be greater than or equal to 0");
  }

  if (min !== null && min !== undefined && max !== null && max !== undefined && min > max) {
    add(diagnostics, path, `${minName} must be less than or equal to ${maxName}`);
  }
}

function numericColumn(col: Column): boolean {
  return col.type === "integer" || col.type === "real" || col.type === "float" || col.type === "number";
}

function validateAudit(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  const crud = new Set(["read", "create", "update", "delete"]);
  for (const [entity, audit] of Object.entries(schema.audit?.entities || {})) {
    if (!tableExists(schema.tables, entity)) {
      add(diagnostics, `audit.entities.${entity}`, `references unknown entity ${JSON.stringify(entity)}`);
      continue;
    }
    if (audit.category && !["data", "workflow", "security", "payment", "system"].includes(audit.category)) {
      add(diagnostics, `audit.entities.${entity}.category`, "must be one of data, workflow, security, payment, or system");
    }
    for (const [index, operation] of (audit.operations || []).entries()) {
      if (!crud.has(operation) && !schema.operations?.[entity]?.[operation]) {
        add(diagnostics, `audit.entities.${entity}.operations.${index}`, `references unknown operation ${entity}.${operation}`);
      }
    }
  }
}

function validateSeed(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  for (const kind of ["reference", "fixtures"] as const) {
    for (const [table, rows] of Object.entries(schema.seed?.[kind] || {})) {
      if (!tableExists(schema.tables, table)) {
        add(diagnostics, `seed.${kind}.${table}`, `references unknown table ${JSON.stringify(table)}`);
        continue;
      }

      const columns = schema.tables[table];
      rows.forEach((row, index) => {
        const path = `seed.${kind}.${table}.${index}`;
        const fields = Object.keys(row);
        if (fields.length === 0) {
          add(diagnostics, path, "must include at least one field");
        }

        for (const [field, value] of Object.entries(row)) {
          const column = columns[field];
          if (!column) {
            add(diagnostics, `${path}.${field}`, `references unknown field ${table}.${field}`);
            continue;
          }
          validateSeedValue(column, value, `${path}.${field}`, diagnostics);
        }

        for (const [field, column] of Object.entries(columns)) {
          if (column.required && column.default === null && !column.auto && !hasOwn(row, field)) {
            add(diagnostics, path, `missing required field ${table}.${field}`);
          }
        }

        if (seedConflictColumns(schema, table, columns, row).length === 0) {
          add(diagnostics, path, "must include a primary-key or unique field so seeding remains idempotent");
        }
      });
    }
  }
}

function hasOwn(row: Record<string, SeedValue>, field: string): boolean {
  return Object.prototype.hasOwnProperty.call(row, field);
}

function seedConflictColumns(schema: Schema, table: string, columns: Record<string, Column>, row: Record<string, SeedValue>): string[] {
  const primaryKey = Object.entries(columns)
    .filter(([field, column]) => column.pk && hasOwn(row, field))
    .map(([field]) => field);
  if (primaryKey.length > 0) return primaryKey;

  const unique = Object.entries(columns)
    .find(([field, column]) => column.unique && hasOwn(row, field));
  if (unique) return [unique[0]];

  for (const constraint of Object.values(normalizeConstraints(schema.constraints, schema.tables)[table] || {})) {
    if (constraint.kind !== "unique" || constraint.condition || constraint.enforcement === "application") continue;
    const fields = constraint.fields.filter(field => field.table === table).map(field => field.field);
    if (fields.length > 0 && fields.length === constraint.fields.length && fields.every(field => hasOwn(row, field))) {
      return fields;
    }
  }

  return [];
}

function validateSeedValue(column: Column, value: SeedValue, path: string, diagnostics: SchemaDiagnostic[]): void {
  if (value === null) {
    if (column.required) add(diagnostics, path, "cannot be null for a required field");
    return;
  }

  const type = column.type.toLowerCase();
  if (type === "integer") {
    if (typeof value !== "number" || !Number.isInteger(value)) add(diagnostics, path, "must be an integer");
    return;
  }
  if (type === "real") {
    if (typeof value !== "number" || !Number.isFinite(value)) add(diagnostics, path, "must be a finite number");
    return;
  }
  if (type === "text") {
    if (typeof value !== "string") add(diagnostics, path, "must be a string");
    return;
  }
  if (type === "blob" && typeof value !== "string") {
    add(diagnostics, path, "must be a string");
  }
}

function validateIntegrations(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  for (const [name, integration] of Object.entries(schema.integrations || {})) {
    const path = `integrations.${name}`;
    if (!integration.provider) {
      add(diagnostics, `${path}.provider`, "is required");
    }
    for (const [envName, env] of Object.entries(integration.env || {})) {
      if (!/^[A-Z][A-Z0-9_]*$/.test(envName)) {
        add(diagnostics, `${path}.env.${envName}`, "must be an uppercase environment variable name");
      }
      if (!env.description) {
        add(diagnostics, `${path}.env.${envName}.description`, "is required");
      }
    }
  }

  const signing = schema.integrations?.webhookEffects?.signing;
  if (!signing) return;
  if (signing.enabled && signing.algorithm !== "sha256") {
    add(diagnostics, "integrations.webhookEffects.signing.algorithm", "must be sha256");
  }
  if (signing.enabled && signing.payload !== "timestamp.body") {
    add(diagnostics, "integrations.webhookEffects.signing.payload", "must be timestamp.body");
  }
  if (!signing.signatureHeader) {
    add(diagnostics, "integrations.webhookEffects.signing.signatureHeader", "is required");
  }
  if (!signing.timestampHeader) {
    add(diagnostics, "integrations.webhookEffects.signing.timestampHeader", "is required");
  }
  if (signing.toleranceSeconds < 1) {
    add(diagnostics, "integrations.webhookEffects.signing.toleranceSeconds", "must be at least 1");
  }
}

function validateEffectContracts(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  const contracts = schema.effectContracts;
  if (!contracts) return;
  for (const [name, template] of Object.entries(contracts.notifications?.templates || {})) {
    if (!template.channel) add(diagnostics, `effectContracts.notifications.templates.${name}.channel`, "is required");
  }
}

function validatePresentation(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  for (const [name, group] of Object.entries(schema.presentation?.groups || {})) {
    if (!group.label) {
      add(diagnostics, `presentation.groups.${name}.label`, "is required");
    }
    if (
      group.displayPriority !== null &&
      group.displayPriority !== undefined &&
      !Number.isFinite(group.displayPriority)
    ) {
      add(diagnostics, `presentation.groups.${name}.displayPriority`, "must be a finite number");
    }
  }
}

function expressionFields(expr: Expr | null | undefined): string[] {
  const fields = new Set<string>();
  function walk(e: Expr | null | undefined): void {
    if (!e) return;
    switch (e._t) {
      case "field":
        fields.add(String(e.name || ""));
        return;
      case "bin":
        walk(e.left as Expr);
        walk(e.right as Expr);
        return;
      case "un":
        walk(e.arg as Expr);
        return;
      default:
        return;
    }
  }
  walk(expr);
  return [...fields].filter(Boolean);
}

function validateConstraintExpr(schema: Schema, entity: string, expr: Expr | null | undefined, path: string, diagnostics: SchemaDiagnostic[]): void {
  if (!expr) {
    add(diagnostics, path, "is required");
    return;
  }
  switch (expr._t) {
    case "field": {
      const name = String(expr.name || "");
      if (!columnExists(schema.tables, entity, name)) {
        add(diagnostics, `${path}.name`, `references unknown field ${entity}.${name}`);
      }
      return;
    }
    case "lit":
      return;
    case "bin":
      validateConstraintExpr(schema, entity, expr.left as Expr, `${path}.left`, diagnostics);
      validateConstraintExpr(schema, entity, expr.right as Expr, `${path}.right`, diagnostics);
      return;
    case "un":
      validateConstraintExpr(schema, entity, expr.arg as Expr, `${path}.arg`, diagnostics);
      return;
    case "rel":
      add(diagnostics, path, "cross-field validation expressions cannot reference related records");
      return;
    case "agg":
      add(diagnostics, path, "cross-field validation expressions cannot use aggregate expressions");
      return;
    default:
      add(diagnostics, path, `has unsupported expression type ${(expr as { _t?: unknown })._t}`);
  }
}

function validateOperations(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  for (const [entity, operations] of Object.entries(schema.operations || {})) {
    if (!tableExists(schema.tables, entity)) {
      add(diagnostics, `operations.${entity}`, `references unknown entity ${JSON.stringify(entity)}`);
      continue;
    }
    for (const [name, operation] of Object.entries(operations)) {
      validateOperation(schema, entity, name, operation, diagnostics);
    }
  }
}

function validateOperation(schema: Schema, entity: string, name: string, operation: Operation, diagnostics: SchemaDiagnostic[]): void {
  const path = `operations.${entity}.${name}`;
  if ((operation as any).workflow !== undefined) {
    add(diagnostics, `${path}.workflow`, "has been replaced by presentation, transitions, and audit");
  }
  validateOperationPolicy(operation, `${path}.policy`, diagnostics);
  validateOperationPresentation(schema, operation, `${path}.presentation`, diagnostics);
  validateOperationTransitions(schema, entity, operation, `${path}.transitions`, diagnostics);
  validateOperationAudit(operation, `${path}.audit`, diagnostics);
  validateAssignmentSet(schema, entity, operation.set || {}, `${path}.set`, diagnostics);
  for (const [index, relationship] of (operation.relationships || []).entries()) {
    validateFieldRef(schema, relationship.field, `${path}.relationships.${index}.field`, diagnostics, entity);
  }
  for (const [index, cascade] of (operation.cascade || []).entries()) {
    validateCascade(schema, entity, cascade, `${path}.cascade.${index}`, diagnostics);
  }
  for (const [index, effect] of (operation.effects || []).entries()) {
    validateEffect(schema, effect, `${path}.effects.${index}`, diagnostics);
  }
  validateGuard(schema, entity, operation.guard, `${path}.guard`, diagnostics);
}

function validateOperationAudit(operation: Operation, path: string, diagnostics: SchemaDiagnostic[]): void {
  const audit = operation.audit;
  if (!audit) return;
  if (audit.category && !["data", "workflow", "security", "payment", "system"].includes(audit.category)) {
    add(diagnostics, `${path}.category`, "must be one of data, workflow, security, payment, or system");
  }
}

function validateOperationPresentation(schema: Schema, operation: Operation, path: string, diagnostics: SchemaDiagnostic[]): void {
  const presentation = operation.presentation;
  if (!presentation) return;
  if (presentation.group && !schema.presentation?.groups?.[presentation.group]) {
    add(diagnostics, `${path}.group`, `references unknown presentation group ${JSON.stringify(presentation.group)}`);
  }
  if (
    presentation.displayPriority !== null &&
    presentation.displayPriority !== undefined &&
    !Number.isFinite(presentation.displayPriority)
  ) {
    add(diagnostics, `${path}.displayPriority`, "must be a finite number");
  }
  if (presentation.confirmation?.severity && !["info", "warning", "danger"].includes(presentation.confirmation.severity)) {
    add(diagnostics, `${path}.confirmation.severity`, "must be one of info, warning, or danger");
  }
}

function validateOperationTransitions(schema: Schema, entity: string, operation: Operation, path: string, diagnostics: SchemaDiagnostic[]): void {
  for (const [index, transition] of (operation.transitions || []).entries()) {
    const transitionPath = `${path}.${index}`;
    validateFieldRef(schema, transition.field, `${transitionPath}.field`, diagnostics, entity);
    if (!transition.from || transition.from.length === 0) {
      add(diagnostics, `${transitionPath}.from`, "must include at least one source value");
    }
    if (!transition.to) {
      add(diagnostics, `${transitionPath}.to`, "is required");
    }
    const fieldName = transition.field?.field;
    const assignment = fieldName ? operation.set?.[fieldName] : null;
    if (fieldName && assignment && assignment.value !== transition.to) {
      add(diagnostics, `${transitionPath}.to`, `does not match operation set value ${JSON.stringify(assignment.value)}`);
    } else if (fieldName && operation.set && !(fieldName in operation.set)) {
      add(diagnostics, `${transitionPath}.field`, `must be set by the operation`);
    }
  }
}

function validateOperationPolicy(operation: Operation, path: string, diagnostics: SchemaDiagnostic[]): void {
  const policy = operation.policy;
  if (!policy) return;
  for (const [index, audience] of (policy.audiences || []).entries()) {
    if (!["anonymous", "customer", "staff", "service", "system"].includes(audience)) {
      add(diagnostics, `${path}.audiences.${index}`, "must be one of anonymous, customer, staff, service, or system");
    }
  }
  if (policy.risk && !["low", "medium", "high"].includes(policy.risk)) {
    add(diagnostics, `${path}.risk`, "must be one of low, medium, or high");
  }
  if ((policy as any).label !== undefined || (policy as any).description !== undefined) {
    add(diagnostics, path, "policy can only contain access/security metadata; use presentation for labels and descriptions");
  }
  if (operation.public && policy.audiences?.length && !policy.audiences.includes("anonymous")) {
    add(diagnostics, `${path}.audiences`, "public operations should include anonymous when audiences are set explicitly");
  }
}

function validateAssignmentSet(schema: Schema, entity: string, set: AssignmentSet, path: string, diagnostics: SchemaDiagnostic[]): void {
  for (const [field, assignment] of Object.entries(set || {})) {
    if (!columnExists(schema.tables, entity, field)) {
      add(diagnostics, `${path}.${field}`, `references unknown field ${entity}.${field}`);
      continue;
    }
    validateAssignment(schema, entity, assignment, `${path}.${field}`, diagnostics);
  }
}

function validateAssignment(schema: Schema, entity: string, assignment: FieldAssignment, path: string, diagnostics: SchemaDiagnostic[]): void {
  const sources = [
    assignment.value !== null && assignment.value !== undefined,
    assignment.now === true,
    assignment.expression !== null && assignment.expression !== undefined,
    assignment.input === true,
    assignment.clear === true,
  ].filter(Boolean).length;
  if (sources !== 1) {
    add(diagnostics, path, "must declare exactly one assignment source: value, now, expression, input, or clear");
  }
  if (assignment.expression) validateGuard(schema, entity, assignment.expression, `${path}.expression`, diagnostics);
}

function validateCascade(schema: Schema, sourceEntity: string, cascade: Cascade, path: string, diagnostics: SchemaDiagnostic[]): void {
  const targetEntity = tableRefName(cascade.entity);
  if (!targetEntity || !tableExists(schema.tables, targetEntity)) {
    add(diagnostics, `${path}.entity`, `references unknown entity ${JSON.stringify(targetEntity || cascade.entity)}`);
    return;
  }
  validateAssignmentSet(schema, targetEntity, cascade.set || {}, `${path}.set`, diagnostics);
  if (!cascade.via) {
    const fallback = `${sourceEntity}_id`;
    if (!columnExists(schema.tables, targetEntity, fallback)) {
      add(diagnostics, `${path}.via`, `is omitted, but ${targetEntity}.${fallback} does not exist`);
    }
    return;
  }

  if (isTableRef(cascade.via)) {
    const junction = cascade.via.table;
    if (!tableExists(schema.tables, junction)) {
      add(diagnostics, `${path}.via`, `references unknown junction entity ${JSON.stringify(junction)}`);
      return;
    }
    for (const field of [`${sourceEntity}_id`, `${targetEntity}_id`]) {
      if (!columnExists(schema.tables, junction, field)) {
        add(diagnostics, `${path}.via`, `junction table ${junction} must include ${field}`);
      }
    }
    return;
  }

  validateFieldRef(schema, cascade.via, `${path}.via`, diagnostics, targetEntity);
}

function isTableRef(ref: TableRef | FieldRef): ref is TableRef {
  return Boolean((ref as TableRef).table && !(ref as FieldRef).field);
}

function tableRefName(ref: TableRef | string | null | undefined): string | null {
  if (!ref) return null;
  if (typeof ref === "string") return ref;
  return ref.table || null;
}

function validateEffect(schema: Schema, effect: Effect, path: string, diagnostics: SchemaDiagnostic[]): void {
  const sources = [Boolean(effect.emit), Boolean(effect.notify), Boolean(effect.call)].filter(Boolean).length;
  if (sources !== 1) add(diagnostics, path, "must declare exactly one effect source: emit, notify, or call");

  if (typeof (effect as any).emit === "string") {
    add(diagnostics, `${path}.emit`, "must reference effectRefs.events, not a raw string");
  } else if (effect.emit && !schema.effectContracts?.events?.[effect.emit.event]) {
    add(diagnostics, `${path}.emit`, `references undeclared event ${JSON.stringify(effect.emit.event)}`);
  }

  const notify = effect.notify;
  if (notify) {
    if (!schema.effectContracts?.notifications?.templates?.[notify.template.template]) {
      add(diagnostics, `${path}.notify.template`, `references undeclared notification template ${JSON.stringify(notify.template.template)}`);
    }
    if (!schema.effectContracts?.notifications?.recipients?.[notify.to.recipient]) {
      add(diagnostics, `${path}.notify.to`, `references undeclared notification recipient ${JSON.stringify(notify.to.recipient)}`);
    }
  }

  const call = effect.call;
  if (call && !schema.effectContracts?.calls?.[call.service]?.[call.action]) {
    add(diagnostics, `${path}.call`, `references undeclared service action ${JSON.stringify(`${call.service}.${call.action}`)}`);
  }
}

function validateGuard(schema: Schema, entity: string, expr: Expr | null, path: string, diagnostics: SchemaDiagnostic[]): void {
  if (!expr) return;
  switch (expr._t) {
    case "field": {
      const name = String(expr.name || "");
      if (!columnExists(schema.tables, entity, name)) {
        add(diagnostics, `${path}.name`, `references unknown field ${entity}.${name}`);
      }
      return;
    }
    case "rel": {
      const rel = String(expr.entity || "");
      const field = String(expr.field || "");
      if (!tableExists(schema.tables, rel)) {
        add(diagnostics, `${path}.entity`, `references unknown related entity ${JSON.stringify(rel)}`);
      } else if (!columnExists(schema.tables, rel, field)) {
        add(diagnostics, `${path}.field`, `references unknown field ${rel}.${field}`);
      }
      const fk = `${rel}_id`;
      if (!columnExists(schema.tables, entity, fk)) {
        add(diagnostics, `${path}.entity`, `requires ${entity}.${fk} so the related ${rel} row can be loaded`);
      }
      return;
    }
    case "lit":
      return;
    case "bin":
      validateGuard(schema, entity, expr.left as Expr, `${path}.left`, diagnostics);
      validateGuard(schema, entity, expr.right as Expr, `${path}.right`, diagnostics);
      return;
    case "un":
      validateGuard(schema, entity, expr.arg as Expr, `${path}.arg`, diagnostics);
      return;
    case "agg":
      add(diagnostics, path, "aggregate guard expressions are not supported by the current service generator");
      return;
    default:
      add(diagnostics, path, `has unsupported expression type ${(expr as { _t?: unknown })._t}`);
  }
}

function validateCommerce(schema: Schema, diagnostics: SchemaDiagnostic[]): void {
  const commerce = schema.commerce;
  if (!commerce?.enabled) return;
  if (commerce.checkout.expiryMinutes < 1) add(diagnostics, "commerce.checkout.expiryMinutes", "must be at least 1");
  if (commerce.checkout.maxQuantity < 1) add(diagnostics, "commerce.checkout.maxQuantity", "must be at least 1");
  if (commerce.checkout.maxLines < 1) add(diagnostics, "commerce.checkout.maxLines", "must be at least 1");
  const inference = inferCommerce(schema);
  diagnostics.push(...inference.diagnostics);
}
