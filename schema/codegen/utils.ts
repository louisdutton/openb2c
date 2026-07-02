import type { AppMetadata, FieldRef, InferredCommerceConfig, OrganizationMetadata, Schema, Tables } from "./types";
import { inferCommerceConfig } from "./inference";

export const TS_TYPE_MAP: Record<string, string> = {
  integer: "number",
  text: "string",
  real: "number",
  blob: "Uint8Array",
};

export function pascalCase(s: string): string {
  return s.replace(/(^|_)(\w)/g, (_, __, c) => c.toUpperCase());
}

export function camelCase(s: string): string {
  return s.replace(/_(\w)/g, (_, c) => c.toUpperCase());
}

export function quoteReserved(name: string): string {
  // SQLite reserved words that need quoting
  const reserved = ["transaction", "order", "group", "index"];
  return reserved.includes(name.toLowerCase()) ? `[${name}]` : name;
}

export const DEFAULT_ORGANIZATION_METADATA: OrganizationMetadata = {
  name: "OpenB2C",
  description: "Generated OpenB2C organization",
  logo: null,
};

export const SYSTEM_DEFAULT_VERSION = "0.1.0";

export const SYSTEM_DEFAULT_PORTS = {
  server: 3085,
  mcp: 3086,
};

type PartialOrganizationMetadata = Partial<OrganizationMetadata>;

function slugify(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/&/g, " and ")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function safeSlug(value: string): string {
  return slugify(value) || slugify(DEFAULT_ORGANIZATION_METADATA.name);
}

export function getAppMetadata(schema: {
  organization?: PartialOrganizationMetadata;
}): AppMetadata {
  const organization = { ...DEFAULT_ORGANIZATION_METADATA, ...(schema.organization ?? {}) };
  const name = organization.name.trim() || DEFAULT_ORGANIZATION_METADATA.name;
  const logo = organization.logo?.src
    ? {
      src: organization.logo.src,
      alt: organization.logo.alt ?? null,
    }
    : null;
  const slug = safeSlug(name);

  return {
    name,
    slug,
    apiTitle: `${name} API`,
    description: organization.description,
    version: SYSTEM_DEFAULT_VERSION,
    defaultPorts: SYSTEM_DEFAULT_PORTS,
    uiTitle: name,
    logo,
  };
}

export function getDefaultDatabasePath(app: AppMetadata): string {
  return `${safeSlug(app.slug)}.db`;
}

export function hasCommerceWorkflow(schema: { tables: Tables; commerce?: { enabled?: boolean } } | Schema): boolean {
  return schema.commerce?.enabled === true;
}

export function hasCommerceBookingAliases(_schema: { commerce?: { enabled?: boolean } } | Schema): boolean {
  return false;
}

export function requiredFieldRef(name: string, ref: FieldRef | null | undefined): FieldRef {
  if (!ref) throw new Error(`commerce.${name} is required when commerce is enabled`);
  return ref;
}

export function getEcommerceConfig(schema: Schema): InferredCommerceConfig | null {
  return inferCommerceConfig(schema);
}

export function openApiEcommerceMetadata(schema: Schema): unknown | undefined {
  const ecommerce = getEcommerceConfig(schema);
  if (!ecommerce) return undefined;
  return {
    enabled: true,
    catalog: ecommerce.catalog,
    order: {
      entity: ecommerce.order.entity,
      user: ecommerce.order.user,
    },
    lineItem: {
      entity: ecommerce.lineItem.entity,
      options: ecommerce.lineItem.options,
    },
    transaction: {
      entity: ecommerce.transaction.entity,
    },
    checkout: ecommerce.checkout,
  };
}
