use openb2c_runtime::Service;
use openb2c_schema::Schema;
use std::{path::PathBuf, process::Command, sync::OnceLock};
pub fn schema() -> Schema {
    static MODEL: OnceLock<Schema> = OnceLock::new();
    MODEL
        .get_or_init(|| {
            let result = Command::new("nix")
                .args([
                    "eval",
                    "--json",
                    "-f",
                    "crates/openb2c-runtime/tests/fixtures/composition.nix",
                ])
                .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .output()
                .expect("Run tests inside devenv shell");
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            serde_json::from_slice(&result.stdout).unwrap()
        })
        .clone()
}
#[allow(dead_code)]
pub fn service() -> Service {
    Service::open(schema(), ":memory:", true).unwrap()
}
