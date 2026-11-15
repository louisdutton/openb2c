use std::{env, fs, path::Path};

fn collect(root: &Path, directory: &Path, entries: &mut Vec<String>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut paths = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(root, &path, entries);
        } else if path.extension().is_some_and(|ext| ext == "nix") {
            let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
            entries.push(format!(
                "({relative:?}, include_str!({:?}))",
                path.to_str().unwrap()
            ));
        }
    }
}

fn main() {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let manifest = Path::new(&manifest);
    // Nix packaging uses a standalone root manifest; Cargo uses the workspace crate.
    let schema = if manifest.join("nix/model").is_dir() {
        manifest.join("nix/model")
    } else {
        manifest.join("../../nix/model")
    }
    .canonicalize()
    .unwrap();
    let mut entries = Vec::new();
    collect(&schema, &schema, &mut entries);
    fs::write(
        Path::new(&env::var_os("OUT_DIR").unwrap()).join("nix_schema.rs"),
        format!("&[{}]", entries.join(",\n")),
    )
    .unwrap();
}
