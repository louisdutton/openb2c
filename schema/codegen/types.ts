export type FieldPrivacy = "public" | "internal" | "sensitive" | "secret";

export interface ColumnMetadata {
  label?: string | null;
  helpText?: string | null;
  placeholder?: string | null;
  format?: string | null;
  displayPriority?: number | null;
  privacy?: FieldPrivacy;
  redact?: boolean;
}

export type RelationshipCardinality = "one" | "many";

export interface FieldRef {
  table: string;
  field: string;
  references: string | null;
}

export interface ColumnRelationshipMetadata {
  label?: string | null;
  description?: string | null;
  cardinality?: RelationshipCardinality;
}

export interface ColumnLengthConstraint {
  min?: number | null;
  max?: number | null;
}

export interface ColumnRangeConstraint {
  min?: number | null;
  max?: number | null;
}

export interface ColumnConstraints {
  enum?: string[];
  pattern?: string | null;
  length?: ColumnLengthConstraint;
  range?: ColumnRangeConstraint;
}

export interface Column {
  type: string;
  pk: boolean;
  auto: boolean;
  required: boolean;
  unique: boolean;
  default: string | null;
  references: string | null;
  metadata?: ColumnMetadata;
  constraints?: ColumnConstraints;
  relationship?: ColumnRelationshipMetadata | null;
}

export type Tables = Record<string, Record<string, Column>>;

export interface DerivedField {
  type: string;
  metadata?: ColumnMetadata;
  dependencies: FieldRef[];
  template?: string | null;
  expression?: Expr | null;
}

export type DerivedFields = Record<string, Record<string, DerivedField>>;

export interface Index {
  columns: string[];
  unique: boolean;
  condition?: Expr | null;
}

export type Indexes = Record<string, Record<string, Index>>;

export type Refs = Record<string, Record<string, FieldRef>>;

export interface TableRef {
  table: string;
}

export interface Relationship {
  field: FieldRef;
}

export type Relationships = Record<string, Record<string, Relationship>>;

export interface Expr {
  _t: "field" | "rel" | "lit" | "bin" | "un" | "agg";
  [key: string]: unknown;
}

export type ConstraintKind = "unique" | "check" | "enum" | "pattern" | "length" | "range";
export type ConstraintEnforcement = "auto" | "database" | "application" | "both";

export interface UniqueConstraint {
  fields: FieldRef[];
  condition?: Expr | null;
  enforcement?: ConstraintEnforcement;
}

export interface CheckConstraint {
  fields: FieldRef[];
  expression: Expr;
  condition?: Expr | null;
  enforcement?: ConstraintEnforcement;
}

export type Constraint = UniqueConstraint | CheckConstraint;

export interface ConstraintGroup {
  unique?: Record<string, UniqueConstraint>;
  check?: Record<string, CheckConstraint>;
}

export type Constraints = Record<string, ConstraintGroup>;

export interface NormalizedConstraint {
  kind: ConstraintKind;
  fields: FieldRef[];
  field?: FieldRef;
  expression?: Expr | null;
  condition?: Expr | null;
  enforcement?: ConstraintEnforcement;
  values?: string[];
  regex?: string;
  min?: number | null;
  max?: number | null;
}

export type NormalizedConstraints = Record<string, Record<string, NormalizedConstraint>>;

export interface FieldAssignment {
  value?: string | number | boolean | null;
  now?: boolean;
  expression?: Expr | null;
  input?: boolean;
  clear?: boolean;
}

export type AssignmentSet = Record<string, FieldAssignment>;

export interface Cascade {
  entity: TableRef;
  via: TableRef | FieldRef | null;
  set: AssignmentSet;
}

export interface EventRef {
  event: string;
}

export interface NotificationTemplateRef {
  template: string;
  channel: string;
}

export interface NotificationRecipientRef {
  recipient: string;
}

export interface CallRef {
  service: string;
  action: string;
}

export interface Effect {
  emit: EventRef | null;
  notify: { template: NotificationTemplateRef; to: NotificationRecipientRef } | null;
  call: CallRef | null;
}

export type AuthAudience = "anonymous" | "customer" | "staff" | "service" | "system";
export type PolicyRisk = "low" | "medium" | "high";
export type AuditCategory = "data" | "workflow" | "security" | "payment" | "system";

export interface OperationPolicyMetadata {
  audiences?: AuthAudience[];
  risk?: PolicyRisk;
}

export interface OperationAuditMetadata {
  required?: boolean;
  category?: AuditCategory;
  reason?: string | null;
  summary?: string | null;
  detail?: string | null;
}

export interface Operation {
  guard: Expr | null;
  relationships: Relationship[];
  public: boolean;
  scope: string | null;
  policy?: OperationPolicyMetadata;
  presentation?: OperationPresentationMetadata;
  audit?: OperationAuditMetadata;
  transitions?: OperationTransitionMetadata[];
  set: AssignmentSet;
  cascade: Cascade[];
  effects: Effect[];
}

export type Operations = Record<string, Record<string, Operation>>;

export interface CommerceOption {
  field: FieldRef | null;
  type: string;
  label: string | null;
  default: string | null;
  choices: string[];
  required: boolean;
  min: number | null;
  max: number | null;
}

export interface CommerceCatalogConfig {
  entity: string;
  label: FieldRef | null;
  description: FieldRef | null;
  price: FieldRef | null;
  capacity: {
    field: FieldRef;
    via: FieldRef | null;
  } | null;
  groupBy: FieldRef[];
  variantFields: FieldRef[];
  availability: {
    field: FieldRef | null;
    available: string;
  };
}

export interface CommerceOrderConfig {
  entity: string;
  user: FieldRef | null;
  status: FieldRef | null;
  amount: FieldRef | null;
  currency: FieldRef | null;
  expiresAt: FieldRef | null;
  paymentReference: FieldRef | null;
  client: FieldRef | null;
  pendingStatus: string;
  paidStatus: string;
  expiredStatus: string;
  cancelledStatus: string;
}

export interface CommerceLineItemConfig {
  entity: string;
  catalogItem: FieldRef | null;
  user: FieldRef | null;
  price: FieldRef | null;
  status: FieldRef | null;
  quantity: FieldRef | null;
  reservedStatus: string;
  fulfilledStatus: string;
  cancelledStatus: string;
  options: Record<string, CommerceOption>;
}

export interface CommerceLinkConfig {
  entity: string;
  order?: FieldRef | null;
  transaction?: FieldRef | null;
  lineItem: FieldRef | null;
}

export interface CommerceTransactionConfig {
  entity: string;
  user: FieldRef | null;
  amount: FieldRef | null;
  type: FieldRef | null;
  status: FieldRef | null;
  reference: FieldRef | null;
  client: FieldRef | null;
  purchaseType: string;
  pendingStatus: string;
  completedStatus: string;
  failedStatus: string;
}

export interface InferredCommerceConfig {
  enabled: boolean;
  catalog: CommerceCatalogConfig;
  order: CommerceOrderConfig;
  lineItem: CommerceLineItemConfig;
  orderLine: CommerceLinkConfig;
  transaction: CommerceTransactionConfig;
  transactionLine: CommerceLinkConfig;
  checkout: {
    currency: string;
    expiryMinutes: number;
    maxQuantity: number;
    maxLines: number;
  };
}

export interface CommerceConfig {
  enabled: boolean;
  checkout: {
    currency: string;
    expiryMinutes: number;
    maxQuantity: number;
    maxLines: number;
  };
}

export interface OrganizationLogoMetadata {
  src: string;
  alt: string | null;
}

export interface OrganizationMetadata {
  name: string;
  description: string;
  logo: OrganizationLogoMetadata | null;
}

export interface AppMetadata {
  name: string;
  slug: string;
  apiTitle: string;
  description: string;
  version: string;
  defaultPorts: {
    server: number;
    mcp: number;
  };
  uiTitle: string;
  logo: OrganizationLogoMetadata | null;
}

export interface AuthRoleMetadata {
  label: string;
  description: string;
  audience: Exclude<AuthAudience, "anonymous">;
  defaultScopes: string[];
  internal: boolean;
}

export interface AuthConfig {
  roles: Record<string, AuthRoleMetadata>;
}

export interface AuditEntityMetadata {
  operations: string[];
  category: AuditCategory;
  reason?: string | null;
}

export interface AuditConfig {
  entities: Record<string, AuditEntityMetadata>;
}

export type SeedValue = string | number | boolean | null;
export type SeedRows = Record<string, SeedValue>[];

export interface SeedConfig {
  reference: Record<string, SeedRows>;
  fixtures: Record<string, SeedRows>;
  applyFixturesByDefault?: boolean;
}

export interface IntegrationEnvVarMetadata {
  description: string;
  requiredInProduction: boolean;
  secret: boolean;
  example?: string | null;
}

export interface IntegrationMetadata {
  provider: string;
  description?: string | null;
  env: Record<string, IntegrationEnvVarMetadata>;
}

export interface WebhookSigningMetadata {
  enabled: boolean;
  algorithm: string;
  payload: string;
  signatureHeader: string;
  timestampHeader: string;
  toleranceSeconds: number;
}

export interface WebhookIntegrationMetadata extends IntegrationMetadata {
  signing: WebhookSigningMetadata;
}

export interface IntegrationsConfig {
  identityEmail: IntegrationMetadata;
  emailEffects: IntegrationMetadata;
  payment: IntegrationMetadata;
  paymentWebhook: IntegrationMetadata;
  webhookEffects: WebhookIntegrationMetadata;
}

export interface EffectEventContract {
  label?: string | null;
}

export interface NotificationTemplateContract {
  channel: string;
  label?: string | null;
}

export interface NotificationRecipientContract {
  label?: string | null;
}

export interface CallActionContract {
  label?: string | null;
}

export interface EffectContracts {
  events: Record<string, EffectEventContract>;
  notifications: {
    templates: Record<string, NotificationTemplateContract>;
    recipients: Record<string, NotificationRecipientContract>;
  };
  calls: Record<string, Record<string, CallActionContract>>;
}

export type WorkflowConfirmationSeverity = "info" | "warning" | "danger";

export interface PresentationGroupMetadata {
  label: string;
  description?: string | null;
  displayPriority?: number | null;
}

export interface PresentationConfig {
  groups: Record<string, PresentationGroupMetadata>;
}

export interface OperationTransitionMetadata {
  field: FieldRef;
  from: string[];
  to: string;
}

export interface OperationConfirmationMetadata {
  required: boolean;
  title?: string | null;
  message?: string | null;
  confirmLabel?: string | null;
  severity?: WorkflowConfirmationSeverity;
}

export interface OperationPresentationMetadata {
  label?: string | null;
  description?: string | null;
  group?: string | null;
  displayPriority?: number | null;
  confirmation?: OperationConfirmationMetadata;
}

export interface ProfileExtension {
  entity: string;
  parentEntity: string;
  field: FieldRef;
}

export interface Schema {
  organization: OrganizationMetadata;
  auth?: AuthConfig;
  audit?: AuditConfig;
  seed?: SeedConfig;
  integrations?: IntegrationsConfig;
  effectContracts?: EffectContracts;
  presentation?: PresentationConfig;
  tables: Tables;
  derived?: DerivedFields;
  constraints?: Constraints;
  refs?: Refs;
  entityRefs?: Record<string, TableRef>;
  relationships?: Relationships;
  profileExtensions?: ProfileExtension[];
  operations: Operations;
  commerce?: CommerceConfig;
}
