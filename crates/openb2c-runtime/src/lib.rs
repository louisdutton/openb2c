//! Shared native runtime copied into each generated application's support crates.
pub mod b2c;
pub mod effects;
pub mod identity;
pub mod migrations;
pub mod passwords;
pub mod service;
mod text;
pub mod web;

pub use service::{AuthContext, Service};
pub use web::{router, serve};

/// Resolve identical storage and fixture settings for the REST server.
pub fn open_from_environment(
    schema: openb2c_schema::Schema,
    root: &std::path::Path,
) -> anyhow::Result<Service> {
    let fixtures = match std::env::var("OPENB2C_APPLY_FIXTURES").as_deref() {
        Ok("true") => true,
        Ok("false") => false,
        Ok(_) => anyhow::bail!("OPENB2C_APPLY_FIXTURES must be true or false"),
        Err(_) => !identity::production() && schema.seed.apply_fixtures_by_default,
    };
    let db = database_path(&schema);
    let migrations = std::env::var_os("MIGRATIONS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("migrations"));
    Service::open_at(schema, db, fixtures, &migrations)
}

pub fn database_path(schema: &openb2c_schema::Schema) -> String {
    std::env::var("DB_PATH")
        .unwrap_or_else(|_| format!("{}.sqlite", text::slug(&schema.organization.name)))
}
