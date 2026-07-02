import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { genMcpServer } from "./mcp";
import { genOpenAPI } from "./openapi";
import { genRuntime } from "./runtime";
import { genRoutes } from "./server";
import { genAppShell } from "./ui";
import type { Schema } from "./types";
import { DEFAULT_ORGANIZATION_METADATA } from "./utils";

const PROJECT_ROOT = join(import.meta.dir, "..", "..");

async function loadExampleSchema(example: string): Promise<Schema> {
  const proc = Bun.spawn(
    ["nix", "eval", "--json", "-f", join(PROJECT_ROOT, "examples", example, "composition.nix")],
    {
      cwd: PROJECT_ROOT,
      stdout: "pipe",
      stderr: "pipe",
    }
  );

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);

  if (exitCode !== 0) {
    throw new Error(`nix eval failed for ${example}: ${stderr}`);
  }

  return JSON.parse(stdout) as Schema;
}

function generatedMetadataArtifacts(schema: Schema): string {
  return [
    genRuntime(schema),
    genRoutes(schema),
    genMcpServer(schema),
    genOpenAPI(schema),
    genAppShell(schema),
  ].join("\n");
}

describe("application metadata", () => {
  test("real example composition generates organization-specific metadata", async () => {
    const duchyOpera = await loadExampleSchema("duchyopera");

    expect(duchyOpera.organization).toMatchObject({
      name: "Duchy Opera",
      description: "The Cornish Opera Company",
      logo: {
        alt: "Duchy Opera",
      },
    });
    expect(duchyOpera.organization.logo?.src.endsWith("examples/duchyopera/logo/duchy-opera-icon.svg")).toBe(true);
    expect(duchyOpera).not.toHaveProperty("service");

    const duchyOpenApi = JSON.parse(genOpenAPI(duchyOpera));
    expect(duchyOpenApi.info).toMatchObject({
      title: "Duchy Opera API",
      version: "0.1.0",
      description: "The Cornish Opera Company",
    });
    expect(duchyOpenApi["x-openb2c-organization"]).toMatchObject({
      name: "Duchy Opera",
      description: "The Cornish Opera Company",
      logo: {
        alt: "Duchy Opera",
      },
    });
    expect(duchyOpenApi.servers).toEqual([{ url: "http://localhost:3085" }]);

    const defaultSchema: Schema = {
      organization: DEFAULT_ORGANIZATION_METADATA,
      tables: {
        note: {
          id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
          name: { type: "text", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        },
      },
      operations: {},
    };
    const defaultOpenApi = JSON.parse(genOpenAPI(defaultSchema));
    expect(defaultOpenApi.info).toMatchObject({
      title: "OpenB2C API",
      version: "0.1.0",
      description: "Generated OpenB2C organization",
    });
    expect(defaultOpenApi["x-openb2c-organization"].logo).toBeNull();
    expect(defaultOpenApi.servers).toEqual([{ url: "http://localhost:3085" }]);

    const duchyArtifacts = generatedMetadataArtifacts(duchyOpera);
    expect(duchyArtifacts).toContain('"databasePath": "duchy-opera.db"');
    expect(duchyArtifacts).toContain('"slug": "duchy-opera"');
    expect(duchyArtifacts).toContain("<title>Duchy Opera</title>");
    expect(duchyArtifacts).not.toContain("OpenB2C API");
    expect(duchyArtifacts).not.toContain("openb2c.db");
    expect(duchyArtifacts).not.toContain("http://localhost:3185");

    const defaultArtifacts = generatedMetadataArtifacts(defaultSchema);
    expect(defaultArtifacts).toContain('"databasePath": "openb2c.db"');
    expect(defaultArtifacts).toContain('"slug": "openb2c"');
    expect(defaultArtifacts).toContain("<title>OpenB2C</title>");
    expect(defaultArtifacts).toContain("http://localhost:3085");
    expect(defaultArtifacts).not.toContain("Duchy Opera API");
    expect(defaultArtifacts).not.toContain("duchy-opera.db");
    expect(defaultArtifacts).not.toContain("opera.db");
  });
});
