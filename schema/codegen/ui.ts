/**
 * Generates index.html app shell for the UI.
 */
import type { Schema } from "./types";
import { getAppMetadata } from "./utils";

function escapeAttr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function genShell(title: string, tagName: "ob-app" | "ob-admin-app", baseHref = "", assetRevision = ""): string {
  const escapedTitle = escapeAttr(title);
  const base = baseHref ? `  <base href="${escapeAttr(baseHref)}" />\n` : "";
  const revision = assetRevision ? `?v=${escapeAttr(assetRevision)}` : "";

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
${base}  <title>${escapedTitle}</title>
  <link rel="stylesheet" href="styles.css${revision}" />
</head>
<body>
  <${tagName} src="openapi.json"></${tagName}>
  <script type="module" src="app.js${revision}"></script>
</body>
</html>
`;
}

export function genPublicAppShell(schema: Schema, assetRevision = ""): string {
  const app = getAppMetadata(schema);

  return genShell(app.uiTitle, "ob-app", "", assetRevision);
}

export function genAdminAppShell(schema: Schema, assetRevision = ""): string {
  const app = getAppMetadata(schema);

  return genShell(`${app.uiTitle} Admin`, "ob-admin-app", "/admin/", assetRevision);
}

export const genAppShell = genPublicAppShell;
