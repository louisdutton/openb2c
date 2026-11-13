//! The evaluated Nix composition is the sole application model.
pub mod contract;
pub mod inference;
pub mod migration;
pub mod sql;
pub mod validate;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type Tables = BTreeMap<String, BTreeMap<String, Column>>;
pub type Operations = BTreeMap<String, BTreeMap<String, Operation>>;
pub type Assignments = BTreeMap<String, Assignment>;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Schema {
    pub organization: Organization,
    pub tables: Tables,
    pub operations: Operations,
    pub constraints: BTreeMap<String, ConstraintGroup>,
    pub derived: BTreeMap<String, BTreeMap<String, DerivedField>>,
    pub seed: Seed,
    // Preserve capability metadata verbatim so no second schema is introduced.
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Organization {
    pub name: String,
    pub description: String,
    pub logo: Option<Value>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SqlType {
    Integer,
    Real,
    #[default]
    Text,
    Blob,
}

impl SqlType {
    pub fn sql(self) -> &'static str {
        match self {
            Self::Integer => "INTEGER",
            Self::Real => "REAL",
            Self::Text => "TEXT",
            Self::Blob => "BLOB",
        }
    }
    pub fn rust(self) -> &'static str {
        match self {
            Self::Integer => "i64",
            Self::Real => "f64",
            Self::Text => "String",
            Self::Blob => "Vec<u8>",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Column {
    #[serde(rename = "type")]
    pub kind: SqlType,
    pub pk: bool,
    pub auto: bool,
    pub required: bool,
    pub unique: bool,
    pub default: Option<String>,
    pub references: Option<String>,
    pub constraints: FieldConstraints,
    pub metadata: Value,
    pub relationship: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FieldConstraints {
    #[serde(rename = "enum")]
    pub values: Vec<String>,
    pub pattern: Option<String>,
    pub length: Option<Bounds>,
    pub range: Option<Bounds>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Bounds {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FieldRef {
    pub table: String,
    pub field: String,
    #[serde(default)]
    pub references: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EntityRef {
    pub table: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "_t")]
pub enum Expr {
    #[serde(rename = "field")]
    Field { name: String },
    #[serde(rename = "rel")]
    Rel {
        entity: String,
        field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<String>,
    },
    #[serde(rename = "lit")]
    Lit { value: Value },
    #[serde(rename = "bin")]
    Bin {
        op: String,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    #[serde(rename = "un")]
    Un { op: String, arg: Box<Expr> },
    #[serde(rename = "agg")]
    Agg {
        op: String,
        rel: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pred: Option<Box<Expr>>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ConstraintGroup {
    pub unique: BTreeMap<String, Constraint>,
    pub check: BTreeMap<String, Constraint>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Constraint {
    pub fields: Vec<FieldRef>,
    pub expression: Option<Expr>,
    pub condition: Option<Expr>,
    pub enforcement: Enforcement,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Enforcement {
    #[default]
    Auto,
    Database,
    Application,
    Both,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Assignment {
    // Nix emits null for unused sources; `clear` explicitly assigns SQL NULL.
    pub value: Option<Value>,
    pub now: Option<bool>,
    pub expression: Option<Expr>,
    pub input: Option<bool>,
    pub clear: Option<bool>,
}

impl Assignment {
    pub fn source_count(&self) -> usize {
        usize::from(self.value.is_some())
            + usize::from(self.now == Some(true))
            + usize::from(self.expression.is_some())
            + usize::from(self.input == Some(true))
            + usize::from(self.clear == Some(true))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Relationship {
    pub field: FieldRef,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Operation {
    pub guard: Option<Expr>,
    pub relationships: Vec<Relationship>,
    pub public: bool,
    pub scope: Option<String>,
    pub policy: Value,
    pub presentation: Value,
    pub audit: Value,
    pub transitions: Vec<Transition>,
    pub set: Assignments,
    pub cascade: Vec<Cascade>,
    pub effects: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Transition {
    pub field: FieldRef,
    pub from: Vec<String>,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Cascade {
    pub entity: EntityRef,
    pub via: Option<Value>,
    pub set: Assignments,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DerivedField {
    #[serde(rename = "type")]
    pub kind: SqlType,
    #[serde(default)]
    pub dependencies: Vec<FieldRef>,
    pub template: Option<String>,
    pub expression: Option<Expr>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Seed {
    pub reference: BTreeMap<String, Vec<BTreeMap<String, Value>>>,
    pub fixtures: BTreeMap<String, Vec<BTreeMap<String, Value>>>,
    pub apply_fixtures_by_default: bool,
}

/// FK syntax is structural; relationship labels never create edges.
pub fn reference(value: &str) -> Option<(&str, &str)> {
    let (table, field) = value.split_once('(')?;
    let field = field.strip_suffix(')')?;
    (identifier(table) && identifier(field)).then_some((table, field))
}

pub fn identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
