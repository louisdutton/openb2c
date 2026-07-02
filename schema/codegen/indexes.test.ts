import { describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { join } from "node:path";
import { genSQL, inferPerformanceIndexes } from "./sql";
import type { Schema } from "./types";

const PROJECT_ROOT = join(import.meta.dir, "..", "..");

async function nixEvalJson(args: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  const proc = Bun.spawn(["nix", "eval", "--impure", "--json", ...args], {
    cwd: PROJECT_ROOT,
    stdout: "pipe",
    stderr: "pipe",
  });

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);

  return { stdout, stderr, exitCode };
}

async function loadExampleSchema(example: string): Promise<Schema> {
  const result = await nixEvalJson(["-f", join(PROJECT_ROOT, "examples", example, "composition.nix")]);
  if (result.exitCode !== 0) {
    throw new Error(`nix eval failed for ${example}: ${result.stderr}`);
  }
  return JSON.parse(result.stdout) as Schema;
}

function applySql(db: Database, sql: string) {
  for (const stmt of sql.split(/;\s*\n/).filter(s => s.trim())) {
    db.run(stmt);
  }
}

describe("ontology indexes", () => {
  test("example compositions infer performance indexes and preserve explicit uniqueness constraints", async () => {
    const duchyOpera = await loadExampleSchema("duchyopera");
    const duchyInferred = inferPerformanceIndexes(duchyOpera.tables);

    expect("indexes" in duchyOpera).toBe(false);
    expect(duchyInferred.ticket?.by_user_status).toEqual({
      columns: ["user_id", "status"],
      unique: false,
    });
    expect(duchyInferred.performance?.by_venue_start).toEqual({
      columns: ["venue_id", "starts_at"],
      unique: false,
    });

    const duchySql = genSQL(duchyOpera.tables, duchyOpera.constraints);
    expect(duchySql).toContain("CREATE INDEX IF NOT EXISTS ticket_by_user_status ON ticket (user_id, status);");
    expect(duchySql).toContain("CREATE INDEX IF NOT EXISTS performance_by_venue_start ON performance (venue_id, starts_at);");
    expect(duchySql).toContain("CREATE UNIQUE INDEX IF NOT EXISTS performance_artist_unique_pair ON performance_artist (performance_id, user_id);");
    expect(duchySql).toContain("CREATE UNIQUE INDEX IF NOT EXISTS transaction_ticket_unique_pair ON transaction_ticket (transaction_id, ticket_id);");
    expect(duchySql).toContain("CREATE UNIQUE INDEX IF NOT EXISTS booking_ticket_unique_pair ON booking_ticket (booking_id, ticket_id);");
    expect(duchySql).toContain("CREATE UNIQUE INDEX IF NOT EXISTS booking_unique_payment_reference ON booking (payment_reference);");
    expect(duchySql).toContain("CREATE UNIQUE INDEX IF NOT EXISTS transaction_unique_reference ON [transaction] (reference);");
    expect(duchySql).toContain("CREATE UNIQUE INDEX IF NOT EXISTS booking_ticket_unique_ticket ON booking_ticket (ticket_id);");
  });

  test("junction table unique indexes reject duplicate relationships", async () => {
    const duchyOpera = await loadExampleSchema("duchyopera");
    const duchyDb = new Database(":memory:");

    try {
      applySql(duchyDb, genSQL(duchyOpera.tables, duchyOpera.constraints));
      duchyDb.run("INSERT INTO performance_artist (performance_id, user_id) VALUES (1, 1)");
      expect(() => {
        duchyDb.run("INSERT INTO performance_artist (performance_id, user_id) VALUES (1, 1)");
      }).toThrow();

      duchyDb.run("INSERT INTO transaction_ticket (transaction_id, ticket_id) VALUES (1, 1)");
      expect(() => {
        duchyDb.run("INSERT INTO transaction_ticket (transaction_id, ticket_id) VALUES (1, 1)");
      }).toThrow();

      duchyDb.run("INSERT INTO booking_ticket (booking_id, ticket_id) VALUES (1, 2)");
      expect(() => {
        duchyDb.run("INSERT INTO booking_ticket (booking_id, ticket_id) VALUES (1, 2)");
      }).toThrow();
      expect(() => {
        duchyDb.run("INSERT INTO booking_ticket (booking_id, ticket_id) VALUES (2, 2)");
      }).toThrow();
    } finally {
      duchyDb.close();
    }
  });
});
