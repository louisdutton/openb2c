import { existsSync, statSync } from "node:fs";
import { extname, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const uiPort = Number(process.env.UI_PORT || "3097");
process.env.PORT ||= process.env.API_PORT || "0";
process.env.DB_PATH ||= resolve(import.meta.dir, "dev.sqlite");
process.env.OPENB2C_APPLY_FIXTURES ||= "true";
process.env.PAYMENT_PROVIDER ||= "local";
process.env.PAYMENT_WEBHOOK_SECRET ||= "dev-secret";
process.env.AUTH_ENABLED ||= "false";

const serverModule = await import(`${pathToFileURL(resolve(import.meta.dir, "generated/server.ts")).href}?dev=${Date.now()}`);
const apiServer = serverModule.server as Bun.Server;
const apiBase = `http://127.0.0.1:${apiServer.port}`;
const uiRoot = resolve(import.meta.dir, "generated/ui");

const uiServer = Bun.serve({
  port: uiPort,
  async fetch(req) {
    const url = new URL(req.url);
    const pathname = normalizePathname(url.pathname);
    const filePath = resolve(uiRoot, `.${pathname}`);
    if (filePath !== uiRoot && !filePath.startsWith(uiRoot + sep)) {
      return new Response("forbidden", { status: 403 });
    }
    if (!existsSync(filePath) || !statSync(filePath).isFile()) {
      return new Response("not found", { status: 404 });
    }
    if (filePath.endsWith(".html")) {
      const html = await Bun.file(filePath).text();
      return new Response(
        html.replace("</head>", `  <script>window.OPENB2C_API_BASE = ${JSON.stringify(apiBase)};</script>\n</head>`),
        { headers: { "content-type": "text/html; charset=utf-8" } },
      );
    }
    return new Response(Bun.file(filePath), { headers: { "content-type": contentType(filePath) } });
  },
});

console.log(`Duchy Opera UI:  http://127.0.0.1:${uiServer.port}/index.html#/performances`);
console.log(`Duchy Opera API: ${apiBase}`);

function normalizePathname(pathname: string): string {
  let decoded = decodeURIComponent(pathname);
  if (decoded === "/") return "/index.html";
  if (decoded.endsWith("/")) decoded += "index.html";
  return decoded;
}

function contentType(filePath: string): string {
  switch (extname(filePath)) {
    case ".css":
      return "text/css; charset=utf-8";
    case ".html":
      return "text/html; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    case ".json":
      return "application/json; charset=utf-8";
    case ".svg":
      return "image/svg+xml";
    default:
      return "application/octet-stream";
  }
}

function shutdown(signal: NodeJS.Signals): void {
  uiServer.stop(true);
  apiServer.stop(true);
  console.log(`shutdown complete: ${signal}`);
}

process.once("SIGINT", shutdown);
process.once("SIGTERM", shutdown);
