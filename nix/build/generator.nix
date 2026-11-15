{
  pkgs,
  lib,
  rustImport,
  toolchain,
  crate2nix,
}: let
  # rust.import builds the root Cargo package. Give the virtual workspace a
  # generator root package without changing the development workspace layout.
  source = lib.fileset.toSource {
    root = ../../.;
    fileset = lib.fileset.unions [../../Cargo.toml ../../Cargo.lock ../../crates ../model];
  };
  crate2nixTools = pkgs.callPackage "${crate2nix}/tools.nix" {};
  vendor = crate2nixTools.internal.vendorSupport {lockFiles = [../../Cargo.lock];};
  workspaceLines = lib.splitString "\n" (builtins.readFile ../../Cargo.toml);
  workspaceEntry = name:
    lib.findFirst
    (line: lib.hasPrefix "${name} = " line)
    (throw "Missing workspace entry: ${name}")
    workspaceLines;
  # crate2nix's rootCrate API requires a single root package. Dependencies keep
  # their original sources; only inherited manifest entries are resolved here.
  standaloneManifest = path:
    lib.concatMapStringsSep "\n" (
      line: let
        inherited = builtins.match "([a-zA-Z0-9_-]+)[.]workspace = true" line;
      in
        if inherited == null
        then line
        else workspaceEntry (builtins.head inherited)
    ) (lib.splitString "\n" (builtins.readFile path));
  manifest = pkgs.writeText "openb2c-generator-Cargo.toml" (
    lib.replaceStrings ["../openb2c-runtime"] ["crates/openb2c-runtime"]
    (standaloneManifest ../../crates/openb2c/Cargo.toml)
    + ''

      [[bin]]
      name = "openb2c"
      path = "crates/openb2c/src/main.rs"
    ''
  );
  schemaManifest =
    pkgs.writeText "openb2c-schema-Cargo.toml"
    (standaloneManifest ../../crates/openb2c-schema/Cargo.toml);
  runtimeManifest = pkgs.writeText "openb2c-runtime-Cargo.toml" (
    lib.replaceStrings ["crates/openb2c-schema"] ["../openb2c-schema"]
    (standaloneManifest ../../crates/openb2c-runtime/Cargo.toml)
  );
  generatorSource = pkgs.runCommand "openb2c-generator-source" {} ''
    mkdir -p "$out"
    cp -R ${source}/. "$out/"
    chmod -R u+w "$out"
    cp "$out/Cargo.toml" "$out/workspace.toml"
    cp "$out/crates/openb2c-schema/Cargo.toml" "$out/workspace-schema.toml"
    cp "$out/crates/openb2c-runtime/Cargo.toml" "$out/workspace-runtime.toml"
    cp ${manifest} "$out/Cargo.toml"
    cp "$out/crates/openb2c/build.rs" "$out/build.rs"
    cp ${schemaManifest} "$out/crates/openb2c-schema/Cargo.toml"
    cp ${runtimeManifest} "$out/crates/openb2c-runtime/Cargo.toml"
    substituteInPlace "$out/crates/openb2c/src/generate.rs" \
      --replace-fail 'include_str!("../../../Cargo.toml")' 'include_str!("../../../workspace.toml")' \
      --replace-fail 'include_str!("../../openb2c-schema/Cargo.toml")' 'include_str!("../../../workspace-schema.toml")' \
      --replace-fail 'include_str!("../../openb2c-runtime/Cargo.toml")' 'include_str!("../../../workspace-runtime.toml")'
    # Resolve the standalone graph from the workspace lock without network
    # access. Dependency dev-targets differ from workspace member dev-targets.
    export CARGO_HOME="$TMPDIR/cargo"
    mkdir -p "$CARGO_HOME"
    cp ${vendor.cargoConfig} "$CARGO_HOME/config.toml"
    cd "$out"
    ${toolchain}/bin/cargo metadata --offline --format-version 1 > /dev/null
  '';
in
  rustImport generatorSource {}
