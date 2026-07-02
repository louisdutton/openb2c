import { describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildInferenceReport, genInferenceJson, genInferenceMarkdown } from "./inference-report";
import type { Schema } from "./types";
import { DEFAULT_ORGANIZATION_METADATA } from "./utils";

const PROJECT_ROOT = join(import.meta.dir, "..", "..");

async function loadExampleSchema(example: string): Promise<Schema> {
  const proc = Bun.spawn(["nix", "eval", "--impure", "--json", "-f", join(PROJECT_ROOT, "examples", example, "composition.nix")], {
    cwd: PROJECT_ROOT,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (exitCode !== 0) throw new Error(`nix eval failed for ${example}: ${stderr}`);
  return JSON.parse(stdout) as Schema;
}

describe("inference report", () => {
  test("explains Duchy Opera commerce, profiles, and record panels", async () => {
    const schema = await loadExampleSchema("duchyopera");
    const report = buildInferenceReport(schema);

    expect(report.summary.commerceEnabled).toBe(true);
    expect(report.commerce.roles.catalog.value).toBe("performance");
    expect(report.commerce.roles.order.value).toBe("booking");
    expect(report.commerce.roles.lineItem.value).toBe("ticket");
    expect(report.commerce.fields["catalog.variantFields"].value).toEqual([
      "performance.starts_at",
      "performance.ends_at",
      "performance.venue_id",
    ]);
    expect(report.commerce.fields["catalog.capacity"].value).toBe("venue.capacity");

    expect(report.profiles).toContainEqual(expect.objectContaining({
      entity: "artist_profile",
      parentEntity: "user",
      field: "user_id",
    }));

    expect(report.surfaces.production.recordPanels).toEqual(expect.arrayContaining([
      expect.objectContaining({ type: "schedule", entity: "rehearsal" }),
      expect.objectContaining({ type: "matrix", entity: "rehearsal_coverage" }),
      expect.objectContaining({ type: "materials", entity: "production_material" }),
    ]));

    const markdown = genInferenceMarkdown(schema);
    expect(markdown).toContain("## Commerce");
    expect(markdown).toContain("- catalog: performance");
    expect(markdown).toContain("artist_profile extends user via user_id");

    const json = JSON.parse(genInferenceJson(schema));
    expect(json.summary.profileExtensions).toBeGreaterThan(0);
  });

  test("commerce inference diagnostics explain missing criteria", () => {
    const schema: Schema = {
      organization: DEFAULT_ORGANIZATION_METADATA,
      tables: {
        user: {
          id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
        },
      },
      operations: {},
      commerce: {
        enabled: true,
        checkout: { currency: "GBP", expiryMinutes: 15, paymentDeadlineMinutes: 30, allowAsyncPayments: false, maxQuantity: 20, maxLines: 50 },
      },
    };

    const report = buildInferenceReport(schema);
    expect(report.diagnostics).toEqual(expect.arrayContaining([
      expect.objectContaining({
        path: "commerce.inference.order",
        message: expect.stringContaining("expected exactly one table with a user FK"),
      }),
      expect.objectContaining({
        path: "commerce.inference.lineItem",
        message: expect.stringContaining("price_pence"),
      }),
    ]));
  });

  test("codegen CLI keeps inference artifacts out of generated files and can explain to stdout", async () => {
    const outDir = mkdtempSync(join(tmpdir(), "openb2c-inference-report-"));
    const explainOutDir = mkdtempSync(join(tmpdir(), "openb2c-inference-report-"));
    const schema: Schema = {
      organization: DEFAULT_ORGANIZATION_METADATA,
      tables: {
        note: {
          id: { type: "integer", pk: true, auto: true, required: false, unique: false, default: null, references: null },
          name: { type: "text", pk: false, auto: false, required: true, unique: false, default: null, references: null },
        },
      },
      operations: {},
    };

    const proc = Bun.spawn(["bun", join(PROJECT_ROOT, "schema", "codegen", "index.ts"), outDir], {
      cwd: PROJECT_ROOT,
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    });
    proc.stdin.write(JSON.stringify(schema));
    proc.stdin.end();

    const [stdout, stderr, exitCode] = await Promise.all([
      new Response(proc.stdout).text(),
      new Response(proc.stderr).text(),
      proc.exited,
    ]);

    expect(exitCode).toBe(0);
    expect(stderr).toBe("");
    expect(stdout).toContain(`wrote ${outDir}/schema.sql`);
    expect(stdout).not.toContain("inference.md");
    expect(stdout).not.toContain("inference.json");
    expect(existsSync(join(outDir, "inference.json"))).toBe(false);
    expect(existsSync(join(outDir, "inference.md"))).toBe(false);

    const explainProc = Bun.spawn(["bun", join(PROJECT_ROOT, "schema", "codegen", "index.ts"), explainOutDir, "--explain"], {
      cwd: PROJECT_ROOT,
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    });
    explainProc.stdin.write(JSON.stringify(schema));
    explainProc.stdin.end();

    const [explainStdout, explainStderr, explainExitCode] = await Promise.all([
      new Response(explainProc.stdout).text(),
      new Response(explainProc.stderr).text(),
      explainProc.exited,
    ]);

    expect(explainExitCode).toBe(0);
    expect(explainStderr).toBe("");
    expect(explainStdout).toContain("# Inference Report");
    expect(explainStdout).not.toContain(`wrote ${explainOutDir}`);
    expect(existsSync(join(explainOutDir, "inference.json"))).toBe(false);
    expect(existsSync(join(explainOutDir, "inference.md"))).toBe(false);
  });
});
