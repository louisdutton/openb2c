import type { Column, ConstraintKind, Constraints, FieldRef, NormalizedConstraint, NormalizedConstraints, Tables } from "./types";

const topLevelConstraintKinds = ["unique", "check"] as const;

export interface FieldConstraintRule {
  enum?: string[];
  minLength?: number;
  maxLength?: number;
  minimum?: number;
  maximum?: number;
  pattern?: string;
}

export function normalizeConstraints(constraints: Constraints = {}, tables: Tables = {}): NormalizedConstraints {
  const normalized: NormalizedConstraints = {};
  for (const [entity, columns] of Object.entries(tables)) {
    for (const [field, column] of Object.entries(columns)) {
      for (const [name, constraint] of columnConstraints(entity, field, column)) {
        normalized[entity] ||= {};
        normalized[entity][name] = constraint;
      }
    }
  }
  for (const [entity, groups] of Object.entries(constraints || {})) {
    for (const kind of topLevelConstraintKinds) {
      const typedConstraints = groups?.[kind] || {};
      for (const [name, constraint] of Object.entries(typedConstraints)) {
        normalized[entity] ||= {};
        normalized[entity][name] = { ...constraint, kind };
      }
    }
  }
  return normalized;
}

function columnConstraints(entity: string, field: string, column: Column): [string, NormalizedConstraint][] {
  const fieldRef: FieldRef = { table: entity, field, references: column.references };
  const constraints = column.constraints || {};
  const result: [string, NormalizedConstraint][] = [];
  if (constraints.enum?.length) {
    result.push([`${field}_values`, { kind: "enum", fields: [fieldRef], field: fieldRef, values: constraints.enum }]);
  }
  if (constraints.pattern) {
    result.push([`${field}_pattern`, { kind: "pattern", fields: [fieldRef], field: fieldRef, regex: constraints.pattern }]);
  }
  if (constraints.length && (constraints.length.min !== null && constraints.length.min !== undefined || constraints.length.max !== null && constraints.length.max !== undefined)) {
    result.push([`${field}_length`, { kind: "length", fields: [fieldRef], field: fieldRef, min: constraints.length.min, max: constraints.length.max }]);
  }
  if (constraints.range && (constraints.range.min !== null && constraints.range.min !== undefined || constraints.range.max !== null && constraints.range.max !== undefined)) {
    result.push([`${field}_range`, { kind: "range", fields: [fieldRef], field: fieldRef, min: constraints.range.min, max: constraints.range.max }]);
  }
  return result;
}

export function fieldConstraintRules(tables: Tables = {}, constraints: Constraints = {}): Record<string, Record<string, FieldConstraintRule>> {
  const rules: Record<string, Record<string, FieldConstraintRule>> = {};
  for (const [entity, entityConstraints] of Object.entries(normalizeConstraints(constraints, tables))) {
    for (const constraint of Object.values(entityConstraints)) {
      if (!constraint.field) continue;
      const field = constraint.field.field;
      rules[entity] ||= {};
      const rule = rules[entity][field] ||= {};
      if (constraint.kind === "enum" && constraint.values?.length) rule.enum = constraint.values;
      if (constraint.kind === "pattern" && constraint.regex) rule.pattern = constraint.regex;
      if (constraint.kind === "length") {
        if (constraint.min !== null && constraint.min !== undefined) {
          rule.minLength = rule.minLength === undefined ? constraint.min : Math.max(rule.minLength, constraint.min);
        }
        if (constraint.max !== null && constraint.max !== undefined) {
          rule.maxLength = rule.maxLength === undefined ? constraint.max : Math.min(rule.maxLength, constraint.max);
        }
      }
      if (constraint.kind === "range") {
        if (constraint.min !== null && constraint.min !== undefined) {
          rule.minimum = rule.minimum === undefined ? constraint.min : Math.max(rule.minimum, constraint.min);
        }
        if (constraint.max !== null && constraint.max !== undefined) {
          rule.maximum = rule.maximum === undefined ? constraint.max : Math.min(rule.maximum, constraint.max);
        }
      }
    }
  }
  return rules;
}

export function inferredConstraintMessage(
  tables: Tables,
  entity: string,
  name: string,
  constraint: NormalizedConstraint,
): string {
  if (constraint.kind === "unique") {
    const labels = constraint.fields.map(field => fieldLabel(tables, field));
    if (labels.length === 1) return `${labels[0]} must be unique.`;
    if (labels.length > 1) return `${labels.join(" + ")} must be unique together.`;
    return `${humanizeName(entity)} record must be unique.`;
  }
  if (constraint.kind === "enum" && constraint.field) {
    return `${fieldLabel(tables, constraint.field)} must be one of: ${(constraint.values || []).join(", ")}.`;
  }
  if (constraint.kind === "pattern" && constraint.field) {
    return `${fieldLabel(tables, constraint.field)} has an invalid format.`;
  }
  if (constraint.kind === "length" && constraint.field) {
    const label = fieldLabel(tables, constraint.field);
    if (constraint.min !== null && constraint.min !== undefined && constraint.max !== null && constraint.max !== undefined) {
      return `${label} must be between ${constraint.min} and ${constraint.max} characters.`;
    }
    if (constraint.min !== null && constraint.min !== undefined) return `${label} must be at least ${constraint.min} characters.`;
    if (constraint.max !== null && constraint.max !== undefined) return `${label} must be at most ${constraint.max} characters.`;
  }
  if (constraint.kind === "range" && constraint.field) {
    const label = fieldLabel(tables, constraint.field);
    if (constraint.min !== null && constraint.min !== undefined && constraint.max !== null && constraint.max !== undefined) {
      return `${label} must be between ${constraint.min} and ${constraint.max}.`;
    }
    if (constraint.min !== null && constraint.min !== undefined) return `${label} must be at least ${constraint.min}.`;
    if (constraint.max !== null && constraint.max !== undefined) return `${label} must be at most ${constraint.max}.`;
  }
  return `${humanizeName(name)} constraint failed.`;
}

function fieldLabel(tables: Tables, field: FieldRef): string {
  const column = tables[field.table]?.[field.field] as Column | undefined;
  return column?.metadata?.label || humanizeName(field.field.replace(/_id$/, ""));
}

function humanizeName(value: string): string {
  const words = value
    .replace(/_id$/, "")
    .split("_")
    .filter(Boolean)
    .map(word => word.toLowerCase() === "id" ? "ID" : word.toLowerCase() === "vip" ? "VIP" : word);
  const text = words.join(" ");
  return text ? text[0].toUpperCase() + text.slice(1) : "Constraint";
}
