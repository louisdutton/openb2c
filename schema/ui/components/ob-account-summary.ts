/**
 * <ob-account-summary> - Signed-in profile and purchase history.
 */
import { ObApi } from "./ob-api";
import { displayName, escapeAttr, escapeHtml, fieldDisplayLabel, fieldFormat, fieldHelpText, fieldLabel, fieldPlaceholder, formatValue, labelFor, orderedSchemaFields, pluralDisplayName, statusClass } from "../format";
import { stylesheetLink } from "../style-link";

const PROFILE_EXCLUDED_FIELDS = new Set(["id", "email", "user_id", "customer_type", "created_at", "updated_at"]);
const PROFILE_FIELD_ORDER = ["name", "phone", "avatar_url"];

type FieldRef = { field: string; references?: string | null };
type CommerceOption = { label?: string | null; field?: FieldRef | null };
type CommerceConfig = {
  catalog?: { entity?: string; label?: FieldRef | null; price?: FieldRef | null };
  order?: {
    entity?: string;
    user?: FieldRef | null;
    status?: FieldRef | null;
    amount?: FieldRef | null;
    expiresAt?: FieldRef | null;
    paymentReference?: FieldRef | null;
  };
  lineItem?: {
    entity?: string;
    catalogItem?: FieldRef | null;
    user?: FieldRef | null;
    price?: FieldRef | null;
    status?: FieldRef | null;
    quantity?: FieldRef | null;
    options?: Record<string, CommerceOption>;
  };
  orderLine?: { entity?: string; order?: FieldRef | null; lineItem?: FieldRef | null };
};

type Purchase = {
  order: Record<string, unknown>;
  lines: Record<string, unknown>[];
  catalog: Map<string, Record<string, unknown>>;
};

type OwnedLine = {
  line: Record<string, unknown>;
  catalog: Map<string, Record<string, unknown>>;
};

export class ObAccountSummary extends HTMLElement {
  private _userId: number | null = null;
  private _profile: Record<string, unknown> | null = null;
  private _purchases: Purchase[] = [];
  private _ownedLines: OwnedLine[] = [];
  private _error = "";
  private _message = "";
  private _saving = false;
  private _onAuthChanged = () => {
    this._profile = null;
    this._purchases = [];
    this._ownedLines = [];
    void this._render();
  };

  constructor() {
    super();
    this.attachShadow({ mode: "open" });
  }

  async connectedCallback() {
    document.addEventListener("ob-auth-changed", this._onAuthChanged as EventListener);
    await this._render();
  }

  disconnectedCallback() {
    document.removeEventListener("ob-auth-changed", this._onAuthChanged as EventListener);
  }

  private async _render() {
    const api = ObApi.instance;
    if (!api) return;
    await api.ready();

    const userId = api.authContext.userId;
    if (userId === null) {
      this.shadowRoot!.innerHTML = "";
      return;
    }
    if (this._userId !== userId || !this._profile) {
      this._userId = userId;
      await this._load(api, userId);
    }

    this.shadowRoot!.innerHTML = `
      ${stylesheetLink()}
      <div class="account-grid">
        ${this._error ? `<div class="error-msg" role="alert">${escapeHtml(this._error)}</div>` : ""}
        ${this._message ? `<div class="success-msg" role="status">${escapeHtml(this._message)}</div>` : ""}
        ${this._renderProfile(api)}
        ${this._renderPurchases(api)}
      </div>
    `;

    this.shadowRoot!.querySelector<HTMLFormElement>('[data-form="profile"]')?.addEventListener("submit", (event) => {
      event.preventDefault();
      void this._saveProfile();
    });
  }

  private async _load(api: ObApi, userId: number) {
    this._error = "";
    this._message = "";
    try {
      const profileRes = await api.request(`/api/users/${userId}`);
      if (!profileRes.ok) {
        this._profile = null;
        this._error = "Could not load account details.";
      } else {
        this._profile = await profileRes.json() as Record<string, unknown>;
      }
      this._purchases = await this._loadPurchases(api, userId);
      this._ownedLines = await this._loadOwnedLines(api, userId);
    } catch {
      this._profile = null;
      this._purchases = [];
      this._ownedLines = [];
      this._error = "Could not load account.";
    }
  }

  private async _loadPurchases(api: ObApi, userId: number): Promise<Purchase[]> {
    const config = api.getEcommerceConfig() as CommerceConfig | null;
    const orderEntity = config?.order?.entity;
    const orderUserField = config?.order?.user?.field || "user_id";
    if (!orderEntity) return [];

    const orders = await this._fetchRows(api, orderEntity, {
      [orderUserField]: String(userId),
      limit: "10",
      sort: "id",
      order: "desc",
    });
    if (orders.length === 0) return [];

    const catalog = await this._catalogMap(api, config);
    const lineRows = await this._lineItemsForOrders(api, config, orders, userId);
    return orders.map((order) => ({
      order,
      lines: lineRows.get(String(order.id)) || [],
      catalog,
    }));
  }

  private async _loadOwnedLines(api: ObApi, userId: number): Promise<OwnedLine[]> {
    const config = api.getEcommerceConfig() as CommerceConfig | null;
    const lineEntity = config?.lineItem?.entity;
    const lineUserField = config?.lineItem?.user?.field;
    if (!lineEntity || !lineUserField) return [];

    const lines = await this._fetchRows(api, lineEntity, {
      [lineUserField]: String(userId),
      limit: "50",
      sort: "id",
      order: "desc",
    });
    if (lines.length === 0) return [];

    const catalog = await this._catalogMap(api, config);
    return lines.map((line) => ({ line, catalog }));
  }

  private async _lineItemsForOrders(
    api: ObApi,
    config: CommerceConfig | null,
    orders: Record<string, unknown>[],
    userId: number,
  ): Promise<Map<string, Record<string, unknown>[]>> {
    const result = new Map<string, Record<string, unknown>[]>();
    const lineEntity = config?.lineItem?.entity;
    const orderLineEntity = config?.orderLine?.entity;
    const orderField = config?.orderLine?.order?.field;
    const lineField = config?.orderLine?.lineItem?.field;
    if (!lineEntity) return result;

    if (orderLineEntity && orderField && lineField) {
      const lineCache = new Map<string, Record<string, unknown>>();
      for (const order of orders) {
        const orderId = String(order.id);
        const links = await this._fetchRows(api, orderLineEntity, { [orderField]: orderId, limit: "50" });
        const lines = await Promise.all(links.map(async (link) => {
          const lineId = String(link[lineField] ?? "");
          if (!lineId) return null;
          if (!lineCache.has(lineId)) {
            const row = await this._fetchRow(api, lineEntity, lineId);
            if (row) lineCache.set(lineId, row);
          }
          return lineCache.get(lineId) || null;
        }));
        result.set(orderId, lines.filter((line): line is Record<string, unknown> => line !== null));
      }
      if ([...result.values()].some((lines) => lines.length > 0)) return result;
    }

    if (orderLineEntity && orderField && lineField) return result;

    const lineUserField = config?.lineItem?.user?.field || "user_id";
    const lines = await this._fetchRows(api, lineEntity, { [lineUserField]: String(userId), limit: "50", sort: "id", order: "desc" });
    if (orders[0]?.id !== undefined) result.set(String(orders[0].id), lines);
    return result;
  }

  private async _catalogMap(api: ObApi, config: CommerceConfig | null): Promise<Map<string, Record<string, unknown>>> {
    try {
      const res = await api.request("/commerce/catalog");
      if (!res.ok) return new Map();
      const data = await res.json() as {
        items?: Record<string, unknown>[];
        lookups?: Record<string, Record<string, string>>;
      };
      const labelField = config?.catalog?.label?.field || "";
      return new Map((data.items || []).map((item) => {
        const lookupLabel = labelField ? data.lookups?.[labelField]?.[String(item[labelField])] : "";
        return [String(item.id), { ...item, __label: lookupLabel || labelFor(item) }];
      }));
    } catch {
      return new Map();
    }
  }

  private async _fetchRows(api: ObApi, entity: string, params: Record<string, string>): Promise<Record<string, unknown>[]> {
    const query = new URLSearchParams(params);
    try {
      const res = await api.request(`/api/${entity}s?${query.toString()}`);
      if (!res.ok) return [];
      const data = await res.json() as { items?: Record<string, unknown>[] };
      return data.items || [];
    } catch {
      return [];
    }
  }

  private async _fetchRow(api: ObApi, entity: string, id: string): Promise<Record<string, unknown> | null> {
    try {
      const res = await api.request(`/api/${entity}s/${encodeURIComponent(id)}`);
      if (!res.ok) return null;
      return await res.json() as Record<string, unknown>;
    } catch {
      return null;
    }
  }

  private _renderProfile(api: ObApi): string {
    const profile = this._profile || {};
    const schema = api.getInputSchema("user") || api.getSchema("user");
    const props = schema?.properties || {};
    const fields = this._editableProfileFields(api);
    return `
      <section class="account-section" aria-labelledby="profile-title">
        <div class="section-header">
          <h2 id="profile-title">Profile</h2>
          ${profile.email ? `<span>${escapeHtml(profile.email)}</span>` : ""}
        </div>
        <form data-form="profile">
          ${fields.length === 0 ? `<p class="empty">No editable profile fields are available.</p>` : fields.map((field) => {
            const prop = props[field];
            const id = `account-${field}`;
            const label = fieldDisplayLabel(field, prop);
            const help = fieldHelpText(prop);
            const placeholder = fieldPlaceholder(prop);
            const describedBy = help ? `${id}-help` : "";
            const common = `id="${escapeAttr(id)}" name="${escapeAttr(field)}" data-field="${escapeAttr(field)}"${describedBy ? ` aria-describedby="${escapeAttr(describedBy)}"` : ""}`;
            const helpMarkup = help ? `<div class="help-text" id="${escapeAttr(describedBy)}">${escapeHtml(help)}</div>` : "";
            return `
              <div class="form-group">
                <label for="${escapeAttr(id)}">${escapeHtml(label)}</label>
                ${Array.isArray(prop?.enum) && prop.enum.length > 0
                  ? `<select ${common}>
                      ${prop.enum.map((choice: unknown) => `<option value="${escapeAttr(choice)}" ${String(choice) === String(profile[field] ?? "") ? "selected" : ""}>${escapeHtml(choice)}</option>`).join("")}
                    </select>`
                  : `<input ${common} value="${escapeAttr(profile[field] ?? "")}" autocomplete="${escapeAttr(this._autocomplete(field, prop))}"${placeholder ? ` placeholder="${escapeAttr(placeholder)}"` : ""} />`}
                ${helpMarkup}
              </div>`;
          }).join("")}
          ${fields.length > 0 ? `
            <div class="actions">
              <button type="submit" class="primary" ${this._saving ? "disabled" : ""}>${this._saving ? "Saving" : "Save changes"}</button>
            </div>
          ` : ""}
        </form>
      </section>
    `;
  }

  private _editableProfileFields(api: ObApi): string[] {
    const schema = api.getInputSchema("user") || api.getSchema("user");
    return orderedSchemaFields(schema)
      .map(([field]) => field)
      .filter((field) => !PROFILE_EXCLUDED_FIELDS.has(field))
      .sort((a, b) => this._profileFieldRank(a) - this._profileFieldRank(b));
  }

  private _renderPurchases(api: ObApi): string {
    const config = api.getEcommerceConfig() as CommerceConfig | null;
    const orderEntity = config?.order?.entity || "order";
    const lineEntity = config?.lineItem?.entity || "item";
    const lineLabel = pluralDisplayName(lineEntity).toLowerCase();
    return `
      <section class="account-section" aria-labelledby="purchases-title">
        <div class="section-header">
          <h2 id="purchases-title">My ${escapeHtml(lineLabel)}</h2>
          ${this._ownedLines.length > 0 ? `<span>${escapeHtml(`${this._ownedLines.length} ${displayName(lineEntity).toLowerCase()}${this._ownedLines.length === 1 ? "" : "s"}`)}</span>` : ""}
        </div>
        ${this._ownedLines.length === 0 ? `<p class="empty">No ${escapeHtml(lineLabel)} yet.</p>` : `
          <ul class="purchase-list ticket-list">
            ${this._ownedLines.map((owned) => this._renderOwnedLine(owned, config)).join("")}
          </ul>
        `}
      </section>
      <section class="account-section" aria-labelledby="orders-title">
        <div class="section-header">
          <h2 id="orders-title">Bookings</h2>
          ${this._purchases.length > 0 ? `<span>${escapeHtml(`${this._purchases.length} ${displayName(orderEntity).toLowerCase()}${this._purchases.length === 1 ? "" : "s"}`)}</span>` : ""}
        </div>
        ${this._purchases.length === 0 ? `<p class="empty">No bookings yet.</p>` : `
          <ul class="purchase-list">
            ${this._purchases.map((purchase) => this._renderPurchase(purchase, config)).join("")}
          </ul>
        `}
      </section>
    `;
  }

  private _renderOwnedLine(owned: OwnedLine, config: CommerceConfig | null): string {
    const line = owned.line;
    const statusField = config?.lineItem?.status?.field || "status";
    const status = line[statusField] === undefined ? "" : String(line[statusField]);
    const badgeClass = statusClass(statusField, status);
    return `
      <li class="purchase-item ticket-item">
        <div class="purchase-summary">
          <div>
            <strong>${escapeHtml(this._lineTitle(line, owned.catalog, config))}</strong>
            <span>${escapeHtml(this._lineMeta(line, config))}</span>
          </div>
          ${status ? `<span class="badge ${escapeAttr(badgeClass)}">${escapeHtml(status)}</span>` : ""}
        </div>
      </li>
    `;
  }

  private _renderPurchase(purchase: Purchase, config: CommerceConfig | null): string {
    const order = purchase.order;
    const orderEntity = config?.order?.entity || "order";
    const statusField = config?.order?.status?.field || "status";
    const status = order[statusField];
    const statusToken = status === undefined ? "" : String(status);
    const statusClassName = statusClass("status", status);
    const meta = this._purchaseMeta(order, config);
    return `
      <li class="purchase-item">
        <div class="purchase-summary">
          <div>
            <strong>${escapeHtml(displayName(orderEntity))} #${escapeHtml(order.id)}</strong>
            ${meta ? `<span>${escapeHtml(meta)}</span>` : ""}
          </div>
          ${statusToken ? `<span class="badge ${escapeAttr(statusClassName)}">${escapeHtml(statusToken)}</span>` : ""}
        </div>
        ${purchase.lines.length > 0 ? `
          <div class="purchase-lines">
            ${purchase.lines.map((line) => this._renderPurchaseLine(line, purchase.catalog, config)).join("")}
          </div>
        ` : ""}
      </li>
    `;
  }

  private _purchaseMeta(row: Record<string, unknown>, config: CommerceConfig | null): string {
    const parts: string[] = [];
    const amountField = config?.order?.amount?.field || "amount_pence";
    const expiresField = config?.order?.expiresAt?.field || "expires_at";
    const referenceField = config?.order?.paymentReference?.field || "payment_reference";
    if (row[amountField] !== undefined) parts.push(formatValue(amountField, row[amountField]));
    if (row[referenceField]) parts.push(String(row[referenceField]));
    if (row.created_at) parts.push(String(row.created_at));
    if (row[expiresField]) parts.push(`expires ${formatValue(expiresField, row[expiresField])}`);
    return parts.filter(Boolean).join(" · ");
  }

  private _renderPurchaseLine(line: Record<string, unknown>, catalog: Map<string, Record<string, unknown>>, config: CommerceConfig | null): string {
    const catalogField = config?.lineItem?.catalogItem?.field;
    const catalogItem = catalogField ? catalog.get(String(line[catalogField])) : null;
    const statusField = config?.lineItem?.status?.field || "status";
    const title = catalogItem ? String(catalogItem.__label || labelFor(catalogItem)) : labelFor(line);
    const meta = this._lineMeta(line, config);
    const status = line[statusField] === undefined ? "" : String(line[statusField]);
    return `
      <div class="purchase-line">
        <div>
          <strong>${escapeHtml(this._lineTitle(line, catalog, config) || title)}</strong>
          ${meta ? `<span>${escapeHtml(meta)}</span>` : ""}
        </div>
        ${status ? `<span>${escapeHtml(status)}</span>` : ""}
      </div>
    `;
  }

  private _lineTitle(line: Record<string, unknown>, catalog: Map<string, Record<string, unknown>>, config: CommerceConfig | null): string {
    const catalogField = config?.lineItem?.catalogItem?.field;
    const catalogItem = catalogField ? catalog.get(String(line[catalogField])) : null;
    return catalogItem ? String(catalogItem.__label || labelFor(catalogItem)) : labelFor(line);
  }

  private _lineMeta(line: Record<string, unknown>, config: CommerceConfig | null): string {
    const priceField = config?.lineItem?.price?.field || "price_pence";
    const quantityField = config?.lineItem?.quantity?.field || "quantity";
    const quantity = Number(line[quantityField] || 1);
    return [
      line.id !== undefined ? `${displayName(config?.lineItem?.entity || "item")} #${line.id}` : "",
      `${Number.isFinite(quantity) && quantity > 1 ? `${quantity} x ` : ""}${line[priceField] !== undefined ? formatValue(priceField, line[priceField]) : ""}`.trim(),
      ...this._lineOptions(line, config),
    ].filter(Boolean).join(" · ");
  }

  private _lineOptions(line: Record<string, unknown>, config: CommerceConfig | null): string[] {
    return Object.entries(config?.lineItem?.options || {}).flatMap(([key, option]) => {
      const field = option.field?.field || key;
      const value = line[field];
      if (value === undefined || value === null || value === "") return [];
      const label = option.label || fieldLabel(field);
      return [`${label}: ${value}`];
    });
  }

  private async _saveProfile() {
    const api = ObApi.instance;
    if (!api || this._userId === null) return;
    const form = this.shadowRoot!.querySelector<HTMLFormElement>('[data-form="profile"]');
    if (!form) return;

    const body: Record<string, unknown> = {};
    const schema = api.getInputSchema("user") || api.getSchema("user");
    const required = new Set<string>(schema?.required || []);
    const props = schema?.properties || {};
    for (const field of this._editableProfileFields(api)) {
      const input = form.querySelector<HTMLInputElement | HTMLSelectElement>(`[data-field="${CSS.escape(field)}"]`);
      if (!input) continue;
      const value = input.value.trim();
      if (value === "" && !required.has(field)) continue;
      const prop = props[field];
      body[field] = prop?.type === "integer" || prop?.type === "number" ? Number(value) : value;
    }

    this._saving = true;
    this._error = "";
    this._message = "";
    await this._render();
    try {
      const res = await api.request(`/api/users/${this._userId}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) {
        this._error = data.error || "Could not save account details.";
      } else {
        this._profile = { ...(this._profile || {}), ...body };
        this._message = "Account details saved.";
      }
    } catch {
      this._error = "Could not save account details.";
    } finally {
      this._saving = false;
      await this._render();
    }
  }

  private _autocomplete(field: string, prop?: any): string {
    const format = fieldFormat(prop);
    if (field === "name") return "name";
    if (format === "phone" || field === "phone") return "tel";
    if (format === "url" || field === "avatar_url") return "url";
    if (format === "email" || field.endsWith("_email")) return "email";
    return "off";
  }

  private _profileFieldRank(field: string): number {
    const index = PROFILE_FIELD_ORDER.indexOf(field);
    return index === -1 ? PROFILE_FIELD_ORDER.length : index;
  }
}

customElements.define("ob-account-summary", ObAccountSummary);
