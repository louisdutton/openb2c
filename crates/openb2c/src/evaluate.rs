use anyhow::{Context, Result, bail};
use std::{fs, io, path::Path, process::Command, time::SystemTime};

const SOURCES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/nix_schema.rs"));

struct Evaluator(std::path::PathBuf);

impl Drop for Evaluator {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn model(path: &Path) -> Result<String> {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    let mut suffix = 0;
    let evaluator = loop {
        let directory = std::env::temp_dir().join(format!(
            "openb2c-evaluator-{}-{unique}-{suffix}",
            std::process::id()
        ));
        match fs::create_dir(&directory) {
            Ok(()) => break Evaluator(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => suffix += 1,
            Err(error) => return Err(error.into()),
        }
    };
    for (relative, source) in SOURCES {
        let target = evaluator.0.join(relative);
        fs::create_dir_all(target.parent().unwrap())?;
        fs::write(target, source)?;
    }
    let result = Command::new("nix-instantiate")
        .args(["--eval", "--strict", "--json"])
        .arg(evaluator.0.join("evaluate.nix"))
        .args(["--argstr", "model"])
        .arg(path)
        .output()
        .context("Run inside `devenv shell` so Nix and NIX_PATH are available")?;
    if !result.status.success() {
        bail!(
            "Nix evaluation failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(String::from_utf8(result.stdout)?)
}
