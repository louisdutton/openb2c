/**
 * <ob-auth-menu> - Small account entrypoint for generated identity sign-in.
 */
import { ObApi } from "./ob-api";
import { escapeAttr, escapeHtml } from "../format";
import { safeReturnTo } from "../route";
import { stylesheetLink } from "../style-link";

export class ObAuthMenu extends HTMLElement {
  static get observedAttributes() {
    return ["placement"];
  }

  private _onAuthChanged = () => {
    this._profile = null;
    this._profileUserId = null;
    void this._render();
  };
  private _profile: Record<string, unknown> | null = null;
  private _profileUserId: number | null = null;
  private _onAuthRequired = (event: Event) => {
    const returnTo = event instanceof CustomEvent && typeof event.detail?.returnTo === "string"
      ? safeReturnTo(event.detail.returnTo)
      : currentRoutePath();
    location.hash = returnTo ? `#/login?return=${encodeURIComponent(returnTo)}` : "#/login";
  };
  private _onClick = (event: Event) => {
    const api = ObApi.instance;
    const signedIn = Boolean(api && api.authContext.userId !== null);

    event.preventDefault();
    location.hash = signedIn ? "#/account" : "#/login";
  };

  constructor() {
    super();
    this.attachShadow({ mode: "open" });
  }

  async connectedCallback() {
    document.addEventListener("ob-auth-changed", this._onAuthChanged as EventListener);
    document.addEventListener("ob-auth-required", this._onAuthRequired as EventListener);
    this.addEventListener("click", this._onClick);
    await this._render();
  }

  attributeChangedCallback() {
    if (this.isConnected) void this._render();
  }

  disconnectedCallback() {
    document.removeEventListener("ob-auth-changed", this._onAuthChanged as EventListener);
    document.removeEventListener("ob-auth-required", this._onAuthRequired as EventListener);
    this.removeEventListener("click", this._onClick);
  }

  private async _render() {
    const api = ObApi.instance;
    if (!api) return;
    await api.ready();

    if (!api.hasIdentityAuth()) {
      this.shadowRoot!.innerHTML = stylesheetLink();
      return;
    }

    const signedIn = api.authContext.userId !== null;
    const profile = signedIn ? await this._profileFor(api, api.authContext.userId!) : null;

    this.shadowRoot!.innerHTML = `
      ${stylesheetLink()}

      <button type="button" class="account-button${signedIn ? " signed-in" : ""}" data-action="account">
        ${signedIn ? this._renderAccountLabel(profile) : "Sign in"}
      </button>
    `;
  }

  private async _profileFor(api: ObApi, userId: number): Promise<Record<string, unknown> | null> {
    if (this._profileUserId === userId) return this._profile;
    this._profileUserId = userId;
    try {
      const res = await api.request(`/api/users/${userId}`);
      this._profile = res.ok ? await res.json() as Record<string, unknown> : null;
    } catch {
      this._profile = null;
    }
    return this._profile;
  }

  private _renderAccountLabel(profile: Record<string, unknown> | null): string {
    const name = displayAccountName(profile);
    const initial = (name.trim()[0] || "A").toUpperCase();
    const avatar = typeof profile?.avatar_url === "string" && profile.avatar_url.trim()
      ? `<img src="${escapeAttr(profile.avatar_url)}" alt="" />`
      : escapeHtml(initial);
    return `
      <span class="account-avatar" aria-hidden="true">${avatar}</span>
      <span class="account-label">${escapeHtml(name)}</span>
    `;
  }
}

function displayAccountName(profile: Record<string, unknown> | null): string {
  const name = typeof profile?.name === "string" ? profile.name.trim() : "";
  if (name && !looksLikeEmail(name)) return name;
  const email = typeof profile?.email === "string" && profile.email.trim()
    ? profile.email.trim()
    : looksLikeEmail(name) ? name : "";
  if (!email) return "Account";
  return compactIdentity(email.split("@")[0] || email);
}

function looksLikeEmail(value: string): boolean {
  return /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(value);
}

function compactIdentity(value: string): string {
  return value.length > 16 ? `${value.slice(0, 13)}...` : value;
}

function currentRoutePath(): string {
  const raw = location.hash.slice(1) || "/";
  const path = raw.split("?")[0] || "/";
  return path.startsWith("/") ? path : `/${path}`;
}

customElements.define("ob-auth-menu", ObAuthMenu);
