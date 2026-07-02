/**
 * <ob-app> - Public generated web app shell.
 */
import { ObApi } from "./ob-api";
import { pluralDisplayName } from "../format";
import { collectionRoutePath } from "../route";
import { apiDescription, apiLogo, apiTitle, focusOutlet, readShellAttributes, renderApiProvider, renderSkipLink, SHELL_OBSERVED_ATTRIBUTES } from "../shell";
import "./ob-auth-menu";
import "./ob-route-outlet";

export class ObApp extends HTMLElement {
  static get observedAttributes() {
    return SHELL_OBSERVED_ATTRIBUTES;
  }

  connectedCallback() {
    this._render();
  }

  attributeChangedCallback() {
    if (this.isConnected) this._render();
  }

  private _render() {
    const attributes = readShellAttributes(this);

    this.innerHTML = `
      ${renderSkipLink()}
      ${renderApiProvider(attributes, `
        <header class="topbar">
          <a class="brand-lockup" href="#/" data-action="home" aria-label="Home">
            <img class="brand-logo" data-role="logo" alt="" hidden />
            <div class="brand-copy">
              <div class="title" data-role="title">OpenB2C</div>
              <div class="description" data-role="description"></div>
            </div>
          </a>
          <div class="top-actions">
            <ob-auth-menu></ob-auth-menu>
            <button class="nav-button" type="button" data-action="commerce" hidden>Browse</button>
          </div>
        </header>
        <ob-route-outlet></ob-route-outlet>
      `)}
    `;

    this.querySelector<HTMLButtonElement>('[data-action="skip"]')?.addEventListener("click", () => {
      focusOutlet(this, "ob-route-outlet");
    });
    this.querySelector<HTMLAnchorElement>('[data-action="home"]')?.addEventListener("click", (event) => {
      event.preventDefault();
      navigateCatalog(catalogPath(ObApi.instance));
    });
    this.querySelector<HTMLButtonElement>('[data-action="commerce"]')?.addEventListener("click", () => {
      navigateCatalog(catalogPath(ObApi.instance));
    });

    const api = this.querySelector("ob-api") as ObApi | null;
    if (!api) return;
    void api.ready().then(() => {
      const titleEl = this.querySelector<HTMLElement>('[data-role="title"]');
      const descriptionEl = this.querySelector<HTMLElement>('[data-role="description"]');
      const logoEl = this.querySelector<HTMLImageElement>('[data-role="logo"]');
      const homeLink = this.querySelector<HTMLAnchorElement>('[data-action="home"]');
      const commerceButton = this.querySelector<HTMLButtonElement>('[data-action="commerce"]');
      const title = apiTitle(api);
      const logo = apiLogo(api);
      const catalogEntity = api.getEcommerceConfig()?.catalog?.entity;
      const routePath = catalogPath(api);
      if (titleEl) titleEl.textContent = title;
      if (descriptionEl) descriptionEl.textContent = apiDescription(api);
      if (homeLink) homeLink.href = `#${routePath}`;
      if (logoEl && logo) {
        logoEl.src = logo.src;
        logoEl.alt = logo.alt || `${title} logo`;
        logoEl.hidden = false;
      } else if (logoEl) {
        logoEl.hidden = true;
        logoEl.removeAttribute("src");
      }
      if (commerceButton) {
        commerceButton.hidden = !api.hasCommerceWorkflow();
        commerceButton.textContent = commerceActionLabel(api, catalogEntity);
      }
    });
  }
}

function navigateCatalog(path: string) {
  document.dispatchEvent(new CustomEvent("ob-commerce-view", { detail: { view: "catalog" } }));
  const target = `#${path}`;
  if (location.hash !== target) location.hash = target;
}

function catalogPath(api?: ObApi | null): string {
  return collectionRoutePath(api?.getEcommerceConfig()?.catalog?.entity);
}

function commerceActionLabel(api: ObApi, catalogEntity?: string): string {
  if (!catalogEntity) return "Browse";
  if (isTemporalCatalogEntity(api, catalogEntity)) return "What's on";
  return `Browse ${pluralDisplayName(catalogEntity)}`;
}

function isTemporalCatalogEntity(api: ObApi, entity: string): boolean {
  const schema = api.getSchema(entity);
  return Object.entries(schema?.properties || {}).some(([field, prop]) => isPublicTemporalField(field, prop));
}

function isPublicTemporalField(field: string, prop: any): boolean {
  if (field === "created_at" || field === "updated_at") return false;
  const format = prop?.["x-openb2c-field"]?.format || prop?.format || "";
  if (format === "date" || format === "date-time") return true;
  if (format === "time") return false;
  return /(^|_)(date|starts_at|start_at|scheduled_at|ends_at|end_at)$/.test(field) || field.endsWith("_date");
}

customElements.define("ob-app", ObApp);
