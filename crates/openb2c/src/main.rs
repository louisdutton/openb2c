mod generate;

use anyhow::{Context, Result, bail};
use openb2c_schema::{Schema, validate::validate};
use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    if let Err(err) = run() {
        eprintln!("openb2c: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if env::args().nth(1).as_deref() == Some("migration") {
        let paths = env::args().skip(2).collect::<Vec<_>>();
        if paths.len() != 2 {
            bail!("Usage: openb2c migration OLD_SCHEMA.json NEW_SCHEMA.json");
        }
        let old: Schema = serde_json::from_str(&fs::read_to_string(&paths[0])?)?;
        let new: Schema = serde_json::from_str(&fs::read_to_string(&paths[1])?)?;
        print!(
            "{}",
            openb2c_schema::migration::stub(&openb2c_schema::migration::plan(&old, &new)?)
        );
        return Ok(());
    }
    let mut composition = None;
    let mut output = None;
    let mut explain = env::var("OPENB2C_EXPLAIN").ok().filter(|s| !s.is_empty());
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: openb2c [--explain[=md|json]] [--out DIR] <composition.nix|->\n\nEvaluates Nix and generates a standalone Rust application in generated.\nUse - to read evaluated JSON from stdin."
                );
                return Ok(());
            }
            "--out" => {
                output = Some(PathBuf::from(
                    args.next().context("--out requires a directory")?,
                ))
            }
            "--explain" => explain = Some("md".into()),
            _ if arg.starts_with("--explain=") => explain = Some(arg[10..].into()),
            _ if arg.starts_with('-') && arg != "-" => bail!("Unknown option {arg}; use --help"),
            _ if composition.is_none() => composition = Some(arg),
            _ => bail!("Unexpected argument {arg}"),
        }
    }
    let explain = match explain.as_deref() {
        None => None,
        Some("md" | "markdown" | "true" | "1") => Some("md"),
        Some("json") => Some("json"),
        Some(other) => bail!("Unknown inference report format {other}; use md or json"),
    };
    let composition = composition.context("Missing composition; use --help")?;
    let input = if composition == "-" {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        input
    } else {
        let path =
            fs::canonicalize(&composition).with_context(|| format!("Cannot read {composition}"))?;
        if output.is_none() {
            output = Some(
                path.parent()
                    .context("Composition has no parent")?
                    .join("generated"),
            );
        }
        let result = Command::new("nix")
            .args(["eval", "--json", "-f"])
            .arg(&path)
            .output()
            .context("Run inside `devenv shell` so Nix and NIX_PATH are available")?;
        if !result.status.success() {
            bail!(
                "Nix evaluation failed:\n{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        String::from_utf8(result.stdout)?
    };
    let schema: Schema = serde_json::from_str(&input).context("Invalid evaluated Nix schema")?;
    let errors = validate(&schema);
    if !errors.is_empty() {
        bail!("Schema validation failed:\n{}", errors.join("\n"));
    }
    let output = output.unwrap_or_else(|| Path::new("generated").to_path_buf());
    generate::write(&schema, &output)?;
    match explain {
        Some("json") => println!(
            "{}",
            serde_json::to_string_pretty(&generate::inference(&schema))?
        ),
        Some(_) => print!("{}", generate::inference_markdown(&schema)),
        None => println!("Generated Rust application in {}", output.display()),
    }
    Ok(())
}
