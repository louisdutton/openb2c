import { existsSync, mkdirSync, statSync } from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const ROOT = resolve(import.meta.dir, "..");
const EXAMPLE = join(ROOT, "examples", "duchyopera");
const GENERATED = join(EXAMPLE, "generated");
const UI_ROOT = join(GENERATED, "ui");
const DB_PATH = process.env.DB_PATH || join(EXAMPLE, "duchyopera.db");
const API_PORT = process.env.PORT || "3085";
const UI_PORT = Number(process.env.UI_PORT || "3097");

async function main() {
  await run(["compose", "examples/duchyopera/composition.nix"]);
  mkdirSync(dirname(DB_PATH), { recursive: true });

  Object.assign(process.env, {
    NODE_ENV: process.env.NODE_ENV || "development",
    DB_PATH,
    PORT: API_PORT,
    AUTH_ENABLED: process.env.AUTH_ENABLED || "false",
    OPENB2C_APPLY_FIXTURES: process.env.OPENB2C_APPLY_FIXTURES || "true",
    PAYMENT_PROVIDER: process.env.PAYMENT_PROVIDER || "local",
    PAYMENT_API_KEY: process.env.PAYMENT_API_KEY || "local-dev",
    PAYMENT_WEBHOOK_SECRET: process.env.PAYMENT_WEBHOOK_SECRET || "local-dev",
    ALLOW_FAKE_PROVIDERS: process.env.ALLOW_FAKE_PROVIDERS || "true",
    EMAIL_PROVIDER: process.env.EMAIL_PROVIDER || "fake",
    EMAIL_FROM: process.env.EMAIL_FROM || "Duchy Opera <dev@example.test>",
  });

  const serverModule = await import(`${pathToFileURL(join(GENERATED, "server.ts")).href}?dev=${Date.now()}`);
  const api = serverModule.server as Bun.Server;
  const apiBase = `http://127.0.0.1:${api.port}`;
  const ui = startStaticUi(apiBase);

  console.log("");
  console.log("Duchy Opera example is running");
  console.log(`API:    ${apiBase}`);
  console.log(`Admin:  http://127.0.0.1:${ui.port}/admin/#/workspaces/production/1`);
  console.log(`Public: http://127.0.0.1:${ui.port}/index.html#/commerce`);
  console.log(`DB:     ${DB_PATH}`);
  console.log("");
  console.log("Press Ctrl+C to stop.");

  const stop = () => {
    ui.stop(true);
    api.stop(true);
    process.exit(0);
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  await new Promise(() => {});
}

async function run(command: string[]) {
  const proc = Bun.spawn(command, {
    cwd: ROOT,
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await proc.exited;
  if (exitCode !== 0) throw new Error(`${command.join(" ")} failed with exit code ${exitCode}`);
}

function startStaticUi(apiBase: string): Bun.Server {
  const root = resolve(UI_ROOT);
  return Bun.serve({
    port: UI_PORT,
    async fetch(req) {
      const url = new URL(req.url);
      const pathname = normalizePathname(url.pathname);
      const filePath = resolve(root, `.${pathname}`);
      if (filePath !== root && !filePath.startsWith(root + sep)) {
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
}

function normalizePathname(pathname: string): string {
  let decoded = decodeURIComponent(pathname);
  if (decoded === "/") return "/index.html";
  if (decoded === "/admin") return "/admin/index.html";
  if (decoded.endsWith("/")) decoded += "index.html";
  return decoded;
}

function contentType(filePath: string): string {
  if (filePath.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filePath.endsWith(".css")) return "text/css; charset=utf-8";
  if (filePath.endsWith(".json")) return "application/json; charset=utf-8";
  if (filePath.endsWith(".svg")) return "image/svg+xml";
  if (filePath.endsWith(".png")) return "image/png";
  return "application/octet-stream";
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
