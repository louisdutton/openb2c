use anyhow::{Context, Result};
use openb2c_schema::{Schema, sql};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};

pub(crate) fn secret_field(name: &str, metadata: &Value) -> bool {
    name.ends_with("_hash")
        || name == "signature"
        || metadata["privacy"] == "secret"
        || metadata["redact"] == true
}

pub fn pascal(name: &str) -> String {
    let value: String = name
        .split('_')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    if value.is_empty() || value.starts_with(|c: char| c.is_ascii_digit()) {
        format!("Entity{value}")
    } else {
        value
    }
}

pub(crate) fn rust_field(name: &str) -> String {
    if name == "_" {
        return "field_".into();
    }
    // Suffixes also cover Rust's non-raw keywords (self, Self, super, crate).
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod",
        "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
        "trait", "true", "type", "unsafe", "use", "where", "while", "abstract", "become", "box",
        "do", "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
    ];
    if KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.into()
    }
}

pub fn inference(schema: &Schema) -> Value {
    openb2c_schema::inference::report(schema)
}

pub fn inference_markdown(schema: &Schema) -> String {
    let mut out = format!("# {} inference\n\n", schema.organization.name);
    for (table, cols) in &schema.tables {
        out.push_str(&format!("## {table}\n\n"));
        for (field, col) in cols {
            if let Some(target) = &col.references {
                out.push_str(&format!("- `{field}` → `{target}`\n"));
            }
        }
        if let Some(ops) = schema.operations.get(table) {
            for (name, op) in ops {
                out.push_str(&format!(
                    "- `{name}`: {}\n",
                    if !op.enabled {
                        "disabled"
                    } else if op.public {
                        "public"
                    } else {
                        "requires authorization"
                    }
                ));
            }
        }
        out.push('\n');
    }
    out
}

pub fn openapi(schema: &Schema) -> Value {
    openb2c_schema::contract::openapi(schema)
}

fn env_example(schema: &Schema) -> String {
    let mut text = "# Generated environment template. Supply secrets through your deployment.\nNODE_ENV=development\nHOST=127.0.0.1\nPORT=3085\nAUTH_ENABLED=true\n# Fixtures follow the model in development and default off in production.\n# OPENB2C_APPLY_FIXTURES=false\nMAX_REQUEST_BODY_BYTES=1048576\nMAX_PAGE_LIMIT=1000\nROUTE_TIMEOUT_MS=30000\n# DB_PATH defaults to the organization slug plus .sqlite in the working directory.\n# Production requires explicit CORS_ORIGINS; identity-enabled apps also need persistent registry keys.\n# CORS_ORIGINS=https://app.example\n".to_string();
    if schema
        .capabilities
        .get("payments")
        .is_some_and(|c| c.settings["feed"] == "starling")
    {
        text.push_str("\n# Account-holder Starling API. Use a read-only token in the business's own deployment.\n# No outgoing payment requests are implemented. Sandbox is the default.\nSTARLING_ENVIRONMENT=sandbox\n# STARLING_ACCESS_TOKEN=\n# BANK_ACCOUNT_ID=\n# STARLING_CATEGORY_UID=\n# BANK_SYNC_FROM=2026-01-01T00:00:00Z\n# BANK_ACCOUNT_NAME=\n# BANK_SORT_CODE=\n# BANK_ACCOUNT_NUMBER=\n# Optional signed webhooks: PEM public key provided by Starling, with actual newlines.\n# STARLING_WEBHOOK_PUBLIC_KEY=\n");
    }
    if schema
        .capabilities
        .get("payments")
        .is_some_and(|c| c.settings["feed"] == "open_banking")
    {
        text.push_str("\n# UK Open Banking AIS resource reader; obtain consent and rotate credentials externally.\n# API base includes its version and aisp prefix, e.g. https://bank.example/open-banking/v4.0/aisp\n# OPEN_BANKING_BASE_URL=\n# OPEN_BANKING_ACCESS_TOKEN_FILE=/run/secrets/bank-access-token\n# OPEN_BANKING_CLIENT_IDENTITY_FILE=/run/secrets/bank-client.pem\n# Optional additional trusted CA; verification remains enabled.\n# OPEN_BANKING_CA_CERT_FILE=/run/secrets/bank-ca.crt\n# BANK_ACCOUNT_ID=\n# BANK_SYNC_FROM=2026-01-01T00:00:00Z\n# BANK_ACCOUNT_NAME=\n# BANK_SORT_CODE=\n# BANK_ACCOUNT_NUMBER=\n");
    }
    if schema.tables.contains_key("identity_registry") {
        text.push_str("# REGISTRY_PRIVATE_KEY=<32-byte Ed25519 seed as hex>\n");
    }
    let mut seen = BTreeSet::new();
    for (kind, contract) in schema
        .metadata
        .get("integrations")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if !schema.integration_active(kind) {
            continue;
        }
        text.push_str(&format!("\n# Integration: {kind}\n"));
        for (name, spec) in contract["env"].as_object().into_iter().flatten() {
            if seen.insert(name.clone()) {
                if let Some(description) = spec["description"].as_str() {
                    text.push_str(&format!("# {}\n", description.replace('\n', " ")));
                }
                if spec["requiredInProduction"] == true {
                    text.push_str("# Required in production when this capability is used.\n");
                }
                text.push_str(&format!("# {name}=\n"));
            }
        }
    }
    text
}

pub fn write(schema: &Schema, output: &Path) -> Result<()> {
    // Compute fallible artifacts first; an invalid schema must leave existing output intact.
    let sql = sql::generate(schema)?;
    let seed = sql::seed_sql(schema, false)?;
    let fixtures = sql::seed_sql(schema, true)?;
    let composition = crate::compile::composition(schema)? + &crate::compile::rest(schema)?;
    let compiled_operations = crate::compile::operations::compile(schema)?;
    let records = crate::compile::records::compile(schema)?;
    let api = crate::compile::api::compile(schema)?;
    let model = schema;
    let main_source = String::from(
        r#"// Generated by OpenB2C. Do not edit.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    const MODEL:&str=include_str!("../schema.json");
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if openb2c_runtime::operations::command(MODEL,root,&args)? {return Ok(());}
    if args.first().map(String::as_str)==Some("--create-api-key") {
        let email=args.get(1).ok_or_else(||anyhow::anyhow!("Usage: --create-api-key EMAIL [SCOPES]"))?;
        openb2c_runtime::identity::provision_api_key(MODEL,root,email,args.get(2).map(String::as_str).unwrap_or("*"))?;
        return Ok(());
    }
    if args.iter().any(|a|a=="--help"||a=="-h") {
        println!("{}",openb2c_runtime::operations::USAGE);
        return Ok(());
    }
    if !args.is_empty() {
        anyhow::bail!("Unknown application arguments; use --help");
    }
    openb2c_runtime::web::serve_at(MODEL,include_str!("../openapi.json"),root).await
}
"#,
    );
    fs::create_dir_all(output.join("src"))?;
    for (name, source) in [
        ("schema.sql", sql), ("seed.sql", seed), ("fixtures.sql", fixtures),
        ("schema.json", serde_json::to_string_pretty(model)?), ("openapi.json", serde_json::to_string_pretty(&openapi(model))?),
        ("src/main.rs", main_source),
        ("Cargo.toml", "# Generated by OpenB2C. Do not edit.\n[package]\nname = \"openb2c-app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nanyhow = \"1\"\nserde = { version = \"1\", features = [\"derive\"] }\ntokio = { version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }\nopenb2c-runtime = { path = \"support/runtime\" }\n".into()),
        (".env.example", env_example(schema)),
    ] { fs::write(output.join(name), source).with_context(|| format!("Writing {name}"))?; }
    for (directory, files) in [
        (
            "schema",
            vec![
                (
                    "src/lib.rs",
                    include_str!("../../openb2c-schema/src/lib.rs"),
                ),
                (
                    "src/backoffice.rs",
                    include_str!("../../openb2c-schema/src/backoffice.rs"),
                ),
                (
                    "src/workflows.rs",
                    include_str!("../../openb2c-schema/src/workflows.rs"),
                ),
                (
                    "src/capabilities.rs",
                    include_str!("../../openb2c-schema/src/capabilities.rs"),
                ),
                (
                    "src/sql.rs",
                    include_str!("../../openb2c-schema/src/sql.rs"),
                ),
                (
                    "src/validate.rs",
                    include_str!("../../openb2c-schema/src/validate.rs"),
                ),
                (
                    "src/inference.rs",
                    include_str!("../../openb2c-schema/src/inference.rs"),
                ),
                (
                    "src/contract.rs",
                    include_str!("../../openb2c-schema/src/contract.rs"),
                ),
                (
                    "src/migration.rs",
                    include_str!("../../openb2c-schema/src/migration.rs"),
                ),
            ],
        ),
        (
            "runtime",
            vec![
                (
                    "src/lib.rs",
                    include_str!("../../openb2c-runtime/src/lib.rs"),
                ),
                (
                    "src/backoffice/mod.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/mod.rs"),
                ),
                (
                    "src/backoffice/accounting.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/accounting.rs"),
                ),
                (
                    "src/backoffice/billing.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/billing.rs"),
                ),
                (
                    "src/backoffice/purchasing.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/purchasing.rs"),
                ),
                (
                    "src/backoffice/documents.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/documents.rs"),
                ),
                (
                    "src/backoffice/banking.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/banking.rs"),
                ),
                (
                    "src/backoffice/reports.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/reports.rs"),
                ),
                (
                    "src/backoffice/sales.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/sales.rs"),
                ),
                (
                    "src/backoffice/history.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/history.rs"),
                ),
                (
                    "src/backoffice/rest.rs",
                    include_str!("../../openb2c-runtime/src/backoffice/rest.rs"),
                ),
                (
                    "src/b2c/mod.rs",
                    include_str!("../../openb2c-runtime/src/b2c/mod.rs"),
                ),
                (
                    "src/b2c/catalog.rs",
                    include_str!("../../openb2c-runtime/src/b2c/catalog.rs"),
                ),
                (
                    "src/b2c/calendar.rs",
                    include_str!("../../openb2c-runtime/src/b2c/calendar.rs"),
                ),
                (
                    "src/b2c/orders.rs",
                    include_str!("../../openb2c-runtime/src/b2c/orders.rs"),
                ),
                (
                    "src/b2c/payments.rs",
                    include_str!("../../openb2c-runtime/src/b2c/payments.rs"),
                ),
                (
                    "src/b2c/transfers.rs",
                    include_str!("../../openb2c-runtime/src/b2c/transfers.rs"),
                ),
                (
                    "src/b2c/bank_feed.rs",
                    include_str!("../../openb2c-runtime/src/b2c/bank_feed.rs"),
                ),
                (
                    "src/b2c/open_banking.rs",
                    include_str!("../../openb2c-runtime/src/b2c/open_banking.rs"),
                ),
                (
                    "src/b2c/starling.rs",
                    include_str!("../../openb2c-runtime/src/b2c/starling.rs"),
                ),
                (
                    "src/b2c/subscriptions.rs",
                    include_str!("../../openb2c-runtime/src/b2c/subscriptions.rs"),
                ),
                (
                    "src/b2c/entitlements.rs",
                    include_str!("../../openb2c-runtime/src/b2c/entitlements.rs"),
                ),
                (
                    "src/b2c/downloads.rs",
                    include_str!("../../openb2c-runtime/src/b2c/downloads.rs"),
                ),
                (
                    "src/b2c/integrations.rs",
                    include_str!("../../openb2c-runtime/src/b2c/integrations.rs"),
                ),
                (
                    "src/b2c/rest.rs",
                    include_str!("../../openb2c-runtime/src/b2c/rest.rs"),
                ),
                (
                    "src/service.rs",
                    include_str!("../../openb2c-runtime/src/service.rs"),
                ),
                (
                    "src/web.rs",
                    include_str!("../../openb2c-runtime/src/web.rs"),
                ),
                (
                    "src/identity.rs",
                    include_str!("../../openb2c-runtime/src/identity.rs"),
                ),
                (
                    "src/transitions.rs",
                    include_str!("../../openb2c-runtime/src/transitions.rs"),
                ),
                (
                    "src/operations.rs",
                    include_str!("../../openb2c-runtime/src/operations.rs"),
                ),
                (
                    "src/accounts.rs",
                    include_str!("../../openb2c-runtime/src/accounts.rs"),
                ),
                (
                    "src/passwords.rs",
                    include_str!("../../openb2c-runtime/src/passwords.rs"),
                ),
                (
                    "src/effects.rs",
                    include_str!("../../openb2c-runtime/src/effects.rs"),
                ),
                (
                    "src/migrations.rs",
                    include_str!("../../openb2c-runtime/src/migrations.rs"),
                ),
                (
                    "src/text.rs",
                    include_str!("../../openb2c-runtime/src/text.rs"),
                ),
            ],
        ),
    ] {
        let target = output.join("support").join(directory);
        fs::create_dir_all(target.join("src"))?;
        for (name, source) in files {
            if directory == "runtime" && !selected_source(schema, name) {
                let path = target.join(name);
                if path.is_file() {
                    fs::remove_file(path)?;
                }
                continue;
            }
            if let Some(parent) = target.join(name).parent() {
                fs::create_dir_all(parent)?;
            }
            let source = if directory == "runtime"
                && ["src/b2c/mod.rs", "src/backoffice/mod.rs"].contains(&name)
            {
                application_modules(schema, name, source)?
            } else if directory == "runtime" && name == "src/service.rs" {
                let (prefix, rest) = source
                    .split_once("// Reference execution begins.\n")
                    .context("Missing reference execution boundary")?;
                let (_, body) = rest
                    .split_once("// Reference execution ends.\n")
                    .context("Missing reference execution boundary")?;
                format!("{prefix}{body}")
            } else if directory == "runtime" && name == "src/lib.rs" {
                format!(
                    "{source}\n#[cfg(feature=\"reference\")]\ncompile_error!(\"Reference execution is test-only; exported services require compiled composition\");\n"
                )
            } else {
                source.to_string()
            };
            fs::write(target.join(name), source)?;
        }
    }
    fs::write(
        output.join("support/schema/Cargo.toml"),
        standalone_manifest(include_str!("../../openb2c-schema/Cargo.toml")),
    )?;
    fs::write(
        output.join("support/runtime/Cargo.toml"),
        standalone_manifest(include_str!("../../openb2c-runtime/Cargo.toml")).replace(
            "default = [\"reference\"]",
            &format!("default = [{}]", selected_features(schema)),
        ),
    )?;
    fs::write(
        output.join("support/runtime/src/composition.rs"),
        composition,
    )?;
    fs::write(
        output.join("support/runtime/src/compiled_operations.rs"),
        compiled_operations,
    )?;
    fs::write(output.join("support/runtime/src/records.rs"), records)?;
    fs::write(output.join("support/runtime/src/compiled_api.rs"), api)?;
    // Remove only artifacts owned by the previous generator. Operator migrations,
    // database files, Cargo.lock and build output survive regeneration.
    for name in [
        "src/models.rs",
        "types.ts",
        "services.ts",
        "runtime.ts",
        "effects.ts",
        "server.ts",
        "mcp.ts",
        "inference.json",
        "inference.md",
    ] {
        let path = output.join(name);
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    for name in [
        "support/runtime/src/commerce.rs",
        "support/runtime/src/mcp.rs",
        "support/runtime/src/ui.rs",
        "support/runtime/assets/app.css",
    ] {
        let path = output.join(name);
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    let assets = output.join("assets");
    if assets.is_dir() {
        for entry in fs::read_dir(assets)? {
            let path = entry?.path();
            if path.is_file() && path.file_stem().is_some_and(|s| s == "logo") {
                fs::remove_file(path)?;
            }
        }
    }
    let old_ui = output.join("ui");
    if old_ui.join("index.html").is_file() {
        fs::remove_dir_all(old_ui)?;
    }
    Ok(())
}

fn selected_source(schema: &Schema, name: &str) -> bool {
    if name == "src/b2c/open_banking.rs" {
        return schema
            .capabilities
            .get("payments")
            .is_some_and(|c| c.settings["feed"] == "open_banking");
    }
    if name == "src/b2c/bank_feed.rs" {
        return schema.capabilities.get("payments").is_some_and(|c| {
            c.settings["feed"] == "starling" || c.settings["feed"] == "open_banking"
        });
    }
    if name == "src/b2c/starling.rs" {
        return schema
            .capabilities
            .get("payments")
            .is_some_and(|c| c.settings["feed"] == "starling");
    }
    let cap = match name {
        "src/b2c/catalog.rs" => "catalog",
        "src/b2c/downloads.rs" => "downloads",
        "src/b2c/entitlements.rs" => "entitlements",
        "src/b2c/orders.rs" => "orders",
        "src/b2c/payments.rs" => "payments",
        "src/b2c/transfers.rs" => "payments",
        "src/b2c/subscriptions.rs" | "src/b2c/calendar.rs" => "subscriptions",
        "src/b2c/integrations.rs" => "order_payments",
        "src/backoffice/accounting.rs" | "src/backoffice/reports.rs" => "accounting",
        "src/backoffice/billing.rs" | "src/backoffice/sales.rs" | "src/backoffice/history.rs" => {
            "billing"
        }
        "src/backoffice/documents.rs" => "documents",
        "src/backoffice/purchasing.rs" => "purchasing",
        "src/backoffice/banking.rs" => "banking",
        "src/b2c/rest.rs" | "src/backoffice/rest.rs" => return false,
        _ => return true,
    };
    schema.capabilities.contains_key(cap)
}

fn selected_features(schema: &Schema) -> String {
    [
        "catalog",
        "downloads",
        "entitlements",
        "order_payments",
        "orders",
        "payments",
        "subscriptions",
        "accounting",
        "billing",
        "documents",
        "purchasing",
        "banking",
        "tax",
    ]
    .into_iter()
    .filter(|cap| schema.capabilities.contains_key(*cap))
    .map(|cap| format!("\"cap_{cap}\""))
    .chain(
        schema
            .capabilities
            .get("payments")
            .is_some_and(|c| c.settings["feed"] == "starling")
            .then_some("\"cap_starling\"".to_string()),
    )
    .chain(
        schema
            .capabilities
            .get("payments")
            .is_some_and(|c| c.settings["feed"] == "open_banking")
            .then_some("\"cap_open_banking\"".to_string()),
    )
    .collect::<Vec<_>>()
    .join(",")
}

fn application_modules(schema: &Schema, name: &str, source: &str) -> Result<String> {
    let (_, body) = source
        .split_once("// Shared implementation follows.\n")
        .context("Missing shared implementation boundary")?;
    let modules: &[(&str, &str)] = if name == "src/b2c/mod.rs" {
        &[
            ("catalog", "catalog"),
            ("downloads", "downloads"),
            ("entitlements", "entitlements"),
            ("order_payments", "integrations"),
            ("orders", "orders"),
            ("payments", "payments"),
            ("payments", "transfers"),
            ("payments", "starling"),
            ("payments", "bank_feed"),
            ("payments", "open_banking"),
            ("subscriptions", "subscriptions"),
            ("subscriptions", "calendar"),
        ]
    } else {
        &[
            ("accounting", "accounting"),
            ("billing", "billing"),
            ("documents", "documents"),
            ("purchasing", "purchasing"),
            ("banking", "banking"),
            ("accounting", "reports"),
            ("billing", "sales"),
            ("billing", "history"),
        ]
    };
    let mut rendered = String::from("//! Compiled application modules. Do not edit.\n");
    for (cap, module) in modules {
        if schema.capabilities.contains_key(*cap)
            && selected_source(schema, &format!("src/b2c/{module}.rs"))
        {
            rendered.push_str(&format!("mod {module};\n"));
        }
    }
    if name == "src/b2c/mod.rs"
        && let Some(payments) = schema.capabilities.get("payments")
    {
        match payments.settings["feed"].as_str() {
            Some("starling") => rendered.push_str("use starling::Starling as SelectedBankFeed;\n"),
            Some("open_banking") => {
                rendered.push_str("use open_banking::OpenBanking as SelectedBankFeed;\n");
                rendered.push_str(&format!(
                    "const OPEN_BANKING_REFERENCE_FIELD: &str = {};\n",
                    serde_json::to_string(
                        payments.settings["reference_field"]
                            .as_str()
                            .unwrap_or("none")
                    )?
                ));
            }
            _ => {}
        }
    }
    if name == "src/backoffice/mod.rs" && schema.capabilities.contains_key("billing") {
        rendered.push_str("pub(crate) use sales::SaleFacts;\n");
    }
    rendered.push_str(body);
    Ok(rendered)
}

// Resolve inherited package/dependency entries from the workspace's actual
// manifest, so generated applications use the same dependency declarations.
fn standalone_manifest(source: &str) -> String {
    let workspace = include_str!("../../../Cargo.toml");
    source
        .lines()
        .map(|line| {
            if let Some(name) = line.strip_suffix(".workspace = true") {
                if name == "openb2c-schema" {
                    return "openb2c-schema = { path = \"../schema\" }".into();
                }
                workspace
                    .lines()
                    .find(|line| line.starts_with(&format!("{name} = ")))
                    .expect("inherited entry must exist in workspace")
                    .to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
