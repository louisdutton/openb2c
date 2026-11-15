# Reusable source generation for an evaluated application schema.
{
  pkgs,
  toolchain,
  crate2nix,
  generator,
  name,
  schema,
}: let
  crate2nixTools = pkgs.callPackage "${crate2nix}/tools.nix" {};
  appVendor = crate2nixTools.internal.vendorSupport {lockFiles = [./generated-app.lock];};
  schemaJson = pkgs.writeText "${name}-schema.json" (builtins.toJSON schema);
  generated = pkgs.runCommand "${name}-generated" {} ''
    ${generator}/bin/openb2c - --out "$out" --explain=json < ${schemaJson} > "$TMPDIR/inference.json"
    ${generator}/bin/openb2c - --out "$out" --explain=md < ${schemaJson} > "$TMPDIR/inference.md"
    cp "$TMPDIR/inference.json" "$TMPDIR/inference.md" "$out/"
    cp ${./generated-app.lock} "$out/Cargo.lock"
    chmod u+w "$out/Cargo.lock"
    # Keep the generated root as the sole workspace member for rust.import.
    substituteInPlace "$out/Cargo.toml" \
      --replace-fail '[workspace]' '[workspace]
    exclude = ["support/schema", "support/runtime"]'
    export CARGO_HOME="$TMPDIR/cargo"
    mkdir -p "$CARGO_HOME"
    cp ${appVendor.cargoConfig} "$CARGO_HOME/config.toml"
    cd "$out"
    ${toolchain}/bin/cargo metadata --offline --format-version 1 > /dev/null
  '';
in
  generated
