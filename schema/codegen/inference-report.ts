import { inferCommerce, inferProfileExtensions, parseReference, type InferenceDiagnostic } from "./inference";
import { normalizeConstraints } from "./constraints";
import type { Column, FieldRef, InferredCommerceConfig, ProfileExtension, Schema } from "./types";

export interface InferenceReport {
  version: 1;
  summary: {
    tables: number;
    relationships: number;
    profileExtensions: number;
    commerceEnabled: boolean;
    diagnostics: number;
  };
  diagnostics: InferenceDiagnostic[];
  entities: Record<string, EntityInferenceReport>;
  profiles: ProfileInferenceReport[];
  commerce: CommerceInferenceReport;
  surfaces: Record<string, SurfaceInferenceReport>;
}

export interface EntityInferenceReport {
  label: InferenceChoice<string | null>;
  foreignKeys: ForeignKeyInference[];
  inboundReferences: ForeignKeyInference[];
  temporalFields: string[];
  moneyFields: string[];
  statusFields: string[];
  uniqueFields: string[];
}

export interface InferenceChoice<T> {
  value: T;
  reason: string;
  evidence: string[];
}

export interface ForeignKeyInference {
  field: string;
  targetEntity: string;
  targetField: string;
  reason: string;
}

export interface ProfileInferenceReport {
  entity: string;
  parentEntity: string;
  field: string;
  reason: string;
  evidence: string[];
}

export interface CommerceInferenceReport {
  enabled: boolean;
  diagnostics: InferenceDiagnostic[];
  roles: Record<string, InferenceChoice<string | null>>;
  fields: Record<string, InferenceChoice<string | string[] | null>>;
  checkout: InferredCommerceConfig["checkout"] | null;
}

export interface SurfaceInferenceReport {
  recordPanels: PanelInferenceReport[];
}

export interface PanelInferenceReport {
  type: "overview" | "list" | "schedule" | "matrix" | "materials" | "profile";
  entity: string;
  reason: string;
  evidence: string[];
}

export function buildInferenceReport(schema: Schema): InferenceReport {
  const entityNames = Object.keys(schema.tables).sort();
  const edges = entityNames.flatMap((entity) => foreignKeys(schema, entity).map((fk) => ({
    sourceEntity: entity,
    sourceField: fk.field,
    targetEntity: fk.target.table,
    targetField: fk.target.field,
  })));
  const entityReports = Object.fromEntries(entityNames.map((entity) => {
    const outbound = edges.filter((edge) => edge.sourceEntity === entity);
    const inbound = edges.filter((edge) => edge.targetEntity === entity);
    const report: EntityInferenceReport = {
      label: labelChoice(schema, entity),
      foreignKeys: outbound.map(edgeReport),
      inboundReferences: inbound.map(edgeReport),
      temporalFields: fieldsMatching(schema, entity, isTemporalField),
      moneyFields: fieldsMatching(schema, entity, isMoneyField),
      statusFields: fieldsMatching(schema, entity, isStatusField),
      uniqueFields: uniqueFields(schema, entity),
    };
    return [entity, report];
  })) as Record<string, EntityInferenceReport>;

  const profileExtensions = inferProfileExtensions(schema);
  const commerce = commerceReport(schema);
  const diagnostics = [...commerce.diagnostics];

  return {
    version: 1,
    summary: {
      tables: entityNames.length,
      relationships: edges.length,
      profileExtensions: profileExtensions.length,
      commerceEnabled: commerce.enabled,
      diagnostics: diagnostics.length,
    },
    diagnostics,
    entities: entityReports,
    profiles: profileExtensions.map(profileReport),
    commerce,
    surfaces: Object.fromEntries(entityNames.map((entity) => [
      entity,
      surfaceReport(schema, entity, edges, profileExtensions),
    ])) as Record<string, SurfaceInferenceReport>,
  };
}

export function genInferenceJson(schema: Schema): string {
  return `${JSON.stringify(buildInferenceReport(schema), null, 2)}\n`;
}

export function genInferenceMarkdown(schema: Schema): string {
  const report = buildInferenceReport(schema);
  const lines: string[] = [
    "# Inference Report",
    "",
    "Developer-facing explanation of what the generator inferred from schema structure.",
    "",
    "## Summary",
    "",
    `- Tables: ${report.summary.tables}`,
    `- Relationships: ${report.summary.relationships}`,
    `- Profile extensions: ${report.summary.profileExtensions}`,
    `- Commerce enabled: ${report.summary.commerceEnabled ? "yes" : "no"}`,
    `- Diagnostics: ${report.summary.diagnostics}`,
  ];

  if (report.diagnostics.length > 0) {
    lines.push("", "## Diagnostics", "");
    for (const diagnostic of report.diagnostics) {
      lines.push(`- ${diagnostic.path}: ${diagnostic.message}`);
    }
  }

  if (report.profiles.length > 0) {
    lines.push("", "## Profile Extensions", "");
    for (const profile of report.profiles) {
      lines.push(`- ${profile.entity} extends ${profile.parentEntity} via ${profile.field}`);
      lines.push(`  Reason: ${profile.reason}`);
      for (const evidence of profile.evidence) lines.push(`  Evidence: ${evidence}`);
    }
  }

  if (report.commerce.enabled) {
    lines.push("", "## Commerce", "");
    for (const [role, choice] of Object.entries(report.commerce.roles)) {
      if (choice.value) lines.push(`- ${role}: ${choice.value} (${choice.reason})`);
    }
    for (const [field, choice] of Object.entries(report.commerce.fields)) {
      const value = Array.isArray(choice.value) ? choice.value.join(", ") : choice.value;
      if (value) lines.push(`- ${field}: ${value} (${choice.reason})`);
    }
  }

  lines.push("", "## Surfaces", "");
  for (const [entity, surface] of Object.entries(report.surfaces)) {
    if (surface.recordPanels.length <= 1) continue;
    lines.push(`### ${entity}`, "");
    for (const panel of surface.recordPanels) {
      lines.push(`- ${panel.type}: ${panel.entity} - ${panel.reason}`);
    }
    lines.push("");
  }

  return `${lines.join("\n").trimEnd()}\n`;
}

function commerceReport(schema: Schema): CommerceInferenceReport {
  const inference = inferCommerce(schema);
  const config = inference.config;
  if (!schema.commerce?.enabled || !config) {
    return {
      enabled: Boolean(schema.commerce?.enabled),
      diagnostics: inference.diagnostics,
      roles: {},
      fields: {},
      checkout: null,
    };
  }

  return {
    enabled: true,
    diagnostics: inference.diagnostics,
    roles: {
      catalog: choice(config.catalog.entity, "catalog target inferred from the line item FK to a priced entity", [
        fieldEvidence(config.lineItem.catalogItem),
        fieldEvidence(config.catalog.price),
      ]),
      order: choice(config.order.entity, "order inferred from checkout status enum, user FK, amount, currency, expiry, and payment reference", [
        fieldEvidence(config.order.user),
        fieldEvidence(config.order.status),
        fieldEvidence(config.order.amount),
        fieldEvidence(config.order.expiresAt),
      ]),
      lineItem: choice(config.lineItem.entity, "line item inferred from reserved/cancelled status, price, and catalog FK", [
        fieldEvidence(config.lineItem.catalogItem),
        fieldEvidence(config.lineItem.status),
        fieldEvidence(config.lineItem.price),
      ]),
      orderLine: choice(config.orderLine.entity, "bridge inferred from FKs to order and line item", [
        fieldEvidence(config.orderLine.order || null),
        fieldEvidence(config.orderLine.lineItem),
      ]),
      transaction: choice(config.transaction.entity, "transaction inferred from payment status enum, user FK, amount, and payment reference", [
        fieldEvidence(config.transaction.user),
        fieldEvidence(config.transaction.status),
        fieldEvidence(config.transaction.amount),
        fieldEvidence(config.transaction.reference),
      ]),
      transactionLine: choice(config.transactionLine.entity, "bridge inferred from FKs to transaction and line item", [
        fieldEvidence(config.transactionLine.transaction || null),
        fieldEvidence(config.transactionLine.lineItem),
      ]),
    },
    fields: {
      "catalog.label": choice(fieldName(config.catalog.label), "label field uses name/title, otherwise a useful FK", [fieldEvidence(config.catalog.label)]),
      "catalog.description": choice(fieldName(config.catalog.description), "description field uses description/summary convention", [fieldEvidence(config.catalog.description)]),
      "catalog.price": choice(fieldName(config.catalog.price), "price field uses price_pence or money metadata", [fieldEvidence(config.catalog.price)]),
      "catalog.groupBy": choice(config.catalog.groupBy.map(fieldName).filter(Boolean) as string[], "catalog grouping follows the inferred label field", config.catalog.groupBy.map(fieldEvidence)),
      "catalog.variantFields": choice(config.catalog.variantFields.map(fieldName).filter(Boolean) as string[], "variant fields are temporal/reference fields ordered by platform convention", config.catalog.variantFields.map(fieldEvidence)),
      "lineItem.options": choice(Object.keys(config.lineItem.options).sort(), "checkout options are non-technical, non-FK line item fields", Object.keys(config.lineItem.options).sort()),
    },
    checkout: config.checkout,
  };
}

function surfaceReport(schema: Schema, entity: string, edges: Array<{ sourceEntity: string; sourceField: string; targetEntity: string; targetField: string }>, profiles: ProfileExtension[]): SurfaceInferenceReport {
  const inbound = edges.filter((edge) => edge.targetEntity === entity);
  const childEntities = new Set(inbound.map((edge) => edge.sourceEntity));
  const panels: PanelInferenceReport[] = [{
    type: "overview",
    entity,
    reason: "every record workspace starts with its own fields and operations",
    evidence: [`table ${entity}`],
  }];

  for (const edge of inbound) {
    const temporal = fieldsMatching(schema, edge.sourceEntity, isTemporalField);
    panels.push({
      type: temporal.length > 0 ? "schedule" : "list",
      entity: edge.sourceEntity,
      reason: temporal.length > 0
        ? "child collection has temporal fields"
        : "child collection is linked by inbound FK",
      evidence: [`${edge.sourceEntity}.${edge.sourceField} -> ${entity}.${edge.targetField}`, ...temporal.map(field => `${edge.sourceEntity}.${field}`)],
    });
  }

  for (const bridge of Object.keys(schema.tables).sort()) {
    if (/call|attendance/.test(bridge)) continue;
    const bridgeEdges = edges.filter((edge) => edge.sourceEntity === bridge && childEntities.has(edge.targetEntity));
    if (bridgeEdges.length < 2) continue;
    panels.push({
      type: "matrix",
      entity: bridge,
      reason: "bridge entity connects two child collections under the same parent",
      evidence: bridgeEdges.map(edge => `${bridge}.${edge.sourceField} -> ${edge.targetEntity}.${edge.targetField}`),
    });
  }

  const material = inbound.find((edge) => /material|document|asset/.test(edge.sourceEntity) && !/version|revision/.test(edge.sourceEntity));
  if (material) {
    const version = edges.find((edge) => /version|revision/.test(edge.sourceEntity) && edge.targetEntity === material.sourceEntity);
    panels.push({
      type: "materials",
      entity: material.sourceEntity,
      reason: version
        ? "material/document child has a version/revision entity"
        : "material/document child exists",
      evidence: [
        `${material.sourceEntity}.${material.sourceField} -> ${entity}.${material.targetField}`,
        version ? `${version.sourceEntity}.${version.sourceField} -> ${version.targetEntity}.${version.targetField}` : "",
      ].filter(Boolean),
    });
  }

  for (const profile of profiles.filter((candidate) => candidate.parentEntity === entity)) {
    panels.push({
      type: "profile",
      entity: profile.entity,
      reason: "one-to-one required unique FK extension of parent entity",
      evidence: [`${profile.entity}.${profile.field.field} -> ${entity}`],
    });
  }

  return {
    recordPanels: dedupePanels(panels),
  };
}

function profileReport(profile: ProfileExtension): ProfileInferenceReport {
  return {
    entity: profile.entity,
    parentEntity: profile.parentEntity,
    field: profile.field.field,
    reason: "table has exactly one required unique FK to parent entity",
    evidence: [
      `${profile.entity}.${profile.field.field} references ${profile.parentEntity}`,
      `${profile.entity}.${profile.field.field} is required`,
      `${profile.entity}.${profile.field.field} is unique by column or unique constraint`,
    ],
  };
}

function labelChoice(schema: Schema, entity: string): InferenceChoice<string | null> {
  const cols = schema.tables[entity] || {};
  if (cols.name) return choice("name", "standard name field", [`${entity}.name exists`]);
  if (cols.title) return choice("title", "fallback title field", [`${entity}.title exists`]);
  const fk = foreignKeys(schema, entity).find(candidate => !["user", "venue"].includes(candidate.target.table)) || foreignKeys(schema, entity)[0];
  if (fk) return choice(fk.field, "no name/title exists, so label falls back to a useful FK", [`${entity}.${fk.field} -> ${fk.target.table}.${fk.target.field}`]);
  return choice(null, "no semantic label field found, so UI falls back to #id", []);
}

function choice<T>(value: T, reason: string, evidence: Array<string | null | undefined>): InferenceChoice<T> {
  return {
    value,
    reason,
    evidence: evidence.filter((item): item is string => Boolean(item)),
  };
}

function foreignKeys(schema: Schema, entity: string): Array<{ field: string; column: Column; target: { table: string; field: string } }> {
  return Object.entries(schema.tables[entity] || {})
    .map(([field, column]) => ({ field, column, target: parseReference(column.references) }))
    .filter((entry): entry is { field: string; column: Column; target: { table: string; field: string } } => Boolean(entry.target));
}

function edgeReport(edge: { sourceEntity: string; sourceField: string; targetEntity: string; targetField: string }): ForeignKeyInference {
  return {
    field: edge.sourceField,
    targetEntity: edge.targetEntity,
    targetField: edge.targetField,
    reason: "foreign key is the structural source of truth",
  };
}

function fieldsMatching(schema: Schema, entity: string, predicate: (field: string, col: Column) => boolean): string[] {
  return Object.entries(schema.tables[entity] || {})
    .filter(([field, col]) => predicate(field, col))
    .map(([field]) => field)
    .sort();
}

function isTemporalField(field: string, col: Column): boolean {
  if (field === "created_at" || field === "updated_at") return false;
  const format = col.metadata?.format || "";
  return format === "date" || format === "time" || format === "date-time" || /(^|_)(date|time|starts_at|ends_at|opens_on|closes_on|expires_at)$/.test(field) || field.endsWith("_at");
}

function isMoneyField(field: string, col: Column): boolean {
  return field.endsWith("_pence") || col.metadata?.format === "money";
}

function isStatusField(field: string, col: Column): boolean {
  return field === "status" || field.endsWith("_status") || Boolean(col.constraints?.enum?.length);
}

function uniqueFields(schema: Schema, entity: string): string[] {
  const constraints = normalizeConstraints(schema.constraints || {}, schema.tables);
  const fromColumns = Object.entries(schema.tables[entity] || {})
    .filter(([, col]) => col.unique)
    .map(([field]) => field);
  const fromConstraints = Object.values(constraints[entity] || {})
    .filter((constraint) => constraint.kind === "unique" && constraint.fields.length === 1)
    .map((constraint) => constraint.fields[0].field);
  return [...new Set([...fromColumns, ...fromConstraints])].sort();
}

function fieldName(ref: FieldRef | null): string | null {
  return ref ? `${ref.table}.${ref.field}` : null;
}

function fieldEvidence(ref: FieldRef | null): string | null {
  if (!ref) return null;
  const reference = ref.references ? ` references ${ref.references}` : "";
  return `${ref.table}.${ref.field}${reference}`;
}

function dedupePanels(panels: PanelInferenceReport[]): PanelInferenceReport[] {
  const seen = new Set<string>();
  return panels.filter((panel) => {
    const key = `${panel.type}:${panel.entity}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
