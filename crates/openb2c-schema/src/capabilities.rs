//! Versioned capability contracts. Roles are explicit exports, never inferred.
use crate::{Enforcement, Schema, SqlType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub version: u32,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub entities: BTreeMap<String, String>,
    #[serde(default)]
    pub settings: Value,
}

pub fn dependencies(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "accounting" | "documents" => &[],
        "billing" | "banking" | "tax" => &["accounting"],
        "purchasing" => &["accounting", "documents"],
        "billing_orders" => &["billing", "orders", "order_payments"],
        "billing_payments" => &["billing", "payments"],
        "billing_subscriptions" => &["billing", "subscriptions"],
        "accounts" | "catalog" => &[],
        "customers" => &["accounts"],
        "orders" => &["customers", "catalog"],
        "payments" | "entitlements" => &["customers"],
        "downloads" => &["entitlements"],
        "password_auth" => &["accounts"],
        "subscriptions" => &["customers", "catalog", "payments"],
        "order_payments" => &["orders", "payments"],
        "order_entitlements" => &["orders", "entitlements"],
        "subscription_entitlements" => &["subscriptions", "entitlements"],
        _ => return None,
    })
}

pub fn entity<'a>(schema: &'a Schema, capability: &str, role: &str) -> Option<&'a str> {
    schema
        .capabilities
        .get(capability)?
        .entities
        .get(role)
        .map(String::as_str)
}

pub(crate) struct EntityContract {
    pub(crate) role: &'static str,
    pub(crate) fields: Vec<(&'static str, SqlType)>,
    pub(crate) nullable: &'static [&'static str],
    pub(crate) read_scope: Option<&'static str>,
    pub(crate) managed: bool,
}
fn contracts(name: &str) -> Vec<EntityContract> {
    use SqlType::{Integer as I, Text as T};
    let contract = |role, fields, nullable, read_scope, managed| EntityContract {
        role,
        fields,
        nullable,
        read_scope,
        managed,
    };
    match name {
        "downloads" => vec![contract(
            "file",
            vec![
                ("id", I),
                ("source_key", T),
                ("name", T),
                ("media_type", T),
                ("content_base64", T),
                ("sha256", T),
                ("size_bytes", I),
                ("resources_json", T),
                ("status", T),
            ],
            &[],
            None,
            true,
        )],
        "password_auth" => vec![
            contract(
                "credential",
                vec![("id", I), ("user_id", I), ("password_hash", T)],
                &[],
                None,
                true,
            ),
            contract(
                "session",
                vec![
                    ("id", I),
                    ("user_id", I),
                    ("token_hash", T),
                    ("expires_at", T),
                    ("revoked", I),
                ],
                &[],
                None,
                true,
            ),
            contract(
                "attempt",
                vec![
                    ("id", I),
                    ("account_key", T),
                    ("peer_key", T),
                    ("created_at", T),
                ],
                &[],
                None,
                true,
            ),
        ],
        "customers" => vec![contract(
            "customer",
            vec![("id", I), ("user_id", I)],
            &[],
            None,
            true,
        )],
        "accounts" => vec![
            contract(
                "staff",
                vec![("id", I), ("user_id", I), ("scopes", T), ("active", I)],
                &[],
                None,
                true,
            ),
            contract(
                "account",
                vec![("id", I), ("email", T), ("name", T)],
                &[],
                None,
                true,
            ),
            contract(
                "credential",
                vec![
                    ("id", I),
                    ("user_id", I),
                    ("key_hash", T),
                    ("key_prefix", T),
                    ("scopes", T),
                    ("active", I),
                ],
                &["scopes", "active"],
                None,
                false,
            ),
        ],
        "catalog" => vec![
            contract(
                "product",
                vec![("id", I), ("code", T), ("name", T), ("active", I)],
                &[],
                None,
                false,
            ),
            contract(
                "price",
                vec![
                    ("id", I),
                    ("product_id", I),
                    ("amount_minor", I),
                    ("currency", T),
                    ("billing_interval", T),
                    ("active", I),
                ],
                &[],
                None,
                false,
            ),
        ],
        "orders" => vec![
            contract(
                "order",
                vec![
                    ("id", I),
                    ("user_id", I),
                    ("status", T),
                    ("amount_minor", I),
                    ("currency", T),
                    ("request_key", T),
                    ("request_json", T),
                    ("created_at", T),
                ],
                &[],
                Some("orders.read"),
                true,
            ),
            contract(
                "item",
                vec![
                    ("id", I),
                    ("order_id", I),
                    ("product_id", I),
                    ("price_id", I),
                    ("product_code", T),
                    ("name", T),
                    ("quantity", I),
                    ("unit_amount_minor", I),
                ],
                &[],
                None,
                true,
            ),
        ],
        "payments" => vec![
            contract(
                "payment",
                vec![
                    ("id", I),
                    ("user_id", I),
                    ("amount_minor", I),
                    ("currency", T),
                    ("provider", T),
                    ("reference", T),
                    ("status", T),
                    ("request_key", T),
                    ("created_at", T),
                    ("bank_reference", T),
                    ("bank_account", T),
                    ("beneficiary", T),
                    ("sort_code", T),
                    ("account_number", T),
                ],
                &[],
                Some("payments.read"),
                true,
            ),
            contract(
                "event",
                vec![
                    ("id", I),
                    ("payment_id", I),
                    ("event_key", T),
                    ("outcome", T),
                    ("result_json", T),
                    ("created_at", T),
                ],
                &[],
                None,
                true,
            ),
            contract(
                "transaction",
                vec![
                    ("id", I),
                    ("source", T),
                    ("account", T),
                    ("transaction_id", T),
                    ("amount_minor", I),
                    ("currency", T),
                    ("direction", T),
                    ("status", T),
                    ("reference", T),
                    ("settled_at", T),
                    ("received_at", T),
                    ("updated_at", T),
                    ("review_reason", T),
                    ("bank_source", T),
                    ("source_reference", T),
                    ("narrative", T),
                    ("disposition", T),
                    ("resolution_note", T),
                ],
                &["settled_at", "review_reason", "resolution_note"],
                None,
                true,
            ),
            contract(
                "allocation",
                vec![
                    ("id", I),
                    ("transaction_id", I),
                    ("payment_id", I),
                    ("amount_minor", I),
                    ("kind", T),
                    ("created_at", T),
                ],
                &[],
                None,
                true,
            ),
            contract(
                "sync",
                vec![
                    ("id", I),
                    ("changes_since", T),
                    ("next_poll_at", T),
                    ("requested", I),
                    ("last_error", T),
                ],
                &["last_error"],
                None,
                true,
            ),
        ],
        "subscriptions" => vec![
            contract(
                "subscription",
                vec![
                    ("id", I),
                    ("user_id", I),
                    ("price_id", I),
                    ("product_code", T),
                    ("amount_minor", I),
                    ("currency", T),
                    ("billing_interval", T),
                    ("status", T),
                    ("cancel_at_period_end", I),
                    ("started_at", T),
                    ("current_period_start", T),
                    ("current_period_end", T),
                    ("cycle", I),
                    ("request_key", T),
                    ("created_at", T),
                ],
                &["started_at", "current_period_start", "current_period_end"],
                Some("subscriptions.read"),
                true,
            ),
            contract(
                "attempt",
                vec![
                    ("id", I),
                    ("subscription_id", I),
                    ("payment_id", I),
                    ("cycle", I),
                    ("attempt", I),
                    ("request_key", T),
                ],
                &[],
                None,
                true,
            ),
        ],
        "entitlements" => vec![contract(
            "entitlement",
            vec![
                ("id", I),
                ("user_id", I),
                ("resource", T),
                ("source_key", T),
                ("status", T),
                ("starts_at", T),
                ("ends_at", T),
            ],
            &["ends_at"],
            Some("entitlements.read"),
            true,
        )],
        "order_payments" => vec![contract(
            "link",
            vec![
                ("id", I),
                ("order_id", I),
                ("payment_id", I),
                ("request_key", T),
            ],
            &[],
            None,
            true,
        )],
        _ => crate::backoffice::contracts(name),
    }
}

pub(crate) fn unique(schema: &Schema, table: &str, fields: &[&str]) -> bool {
    (fields.len() == 1
        && schema
            .tables
            .get(table)
            .and_then(|t| t.get(fields[0]))
            .is_some_and(|c| c.pk || c.unique))
        || schema.constraints.get(table).is_some_and(|g| {
            g.unique.values().any(|c| {
                c.condition.is_none()
                    && c.enforcement != Enforcement::Application
                    && c.fields
                        .iter()
                        .map(|f| f.field.as_str())
                        .collect::<BTreeSet<_>>()
                        == fields.iter().copied().collect()
            })
        })
}

pub fn validate(schema: &Schema) -> Vec<String> {
    let mut errors = Vec::new();
    let mut exports = BTreeMap::new();
    for (name, capability) in &schema.capabilities {
        let path = format!("capabilities.{name}");
        let Some(required) = dependencies(name) else {
            errors.push(format!("{path}: unknown capability"));
            continue;
        };
        if capability.version
            != if ["customers", "password_auth", "billing", "payments"].contains(&name.as_str()) {
                2
            } else {
                1
            }
        {
            errors.push(format!(
                "{path}: unsupported contract version {}",
                capability.version
            ));
        }
        if capability
            .requires
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != required.iter().copied().collect()
        {
            errors.push(format!("{path}.requires: expected {}", required.join(", ")));
        }
        for dependency in &capability.requires {
            if !schema.capabilities.contains_key(dependency) {
                errors.push(format!("{path}: missing dependency {dependency}"));
            }
        }
        if name == "payments"
            && ![
                json!({"provider":"bank_transfer","feed":"simulated"}),
                json!({"provider":"bank_transfer","feed":"starling"}),
                json!({"provider":"bank_transfer","feed":"open_banking","reference_field":"none"}),
                json!({"provider":"bank_transfer","feed":"open_banking","reference_field":"transaction_information"}),
                json!({"provider":"bank_transfer","feed":"open_banking","reference_field":"transaction_reference"}),
            ]
            .contains(&capability.settings)
        {
            errors.push(format!("{path}.settings: expected provider = bank_transfer with simulated, starling, or open_banking feed and a valid compiled reference field"));
        } else if !["payments", "billing", "tax"].contains(&name.as_str())
            && !capability.settings.is_null()
            && capability.settings != json!({})
        {
            errors.push(format!("{path}.settings: unsupported settings"));
        }
        let expected = contracts(name);
        let roles: BTreeSet<_> = expected.iter().map(|e| e.role).collect();
        if capability
            .entities
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != roles
        {
            errors.push(format!(
                "{path}.entities: expected roles {}",
                roles.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        for contract in expected {
            let Some(table) = capability.entities.get(contract.role) else {
                continue;
            };
            if let Some(previous) = exports.insert(table, format!("{name}.{}", contract.role)) {
                errors.push(format!(
                    "{path}: entity {table} is already owned by {previous}"
                ));
            }
            let Some(columns) = schema.tables.get(table) else {
                errors.push(format!("{path}.{}: missing entity {table}", contract.role));
                continue;
            };
            for (field, kind) in &contract.fields {
                match columns.get(*field) {
                    Some(c)
                        if c.kind == *kind
                            && (if contract.nullable.contains(field) {
                                !c.required && !c.pk
                            } else {
                                c.required || c.pk
                            }) => {}
                    _ => errors.push(format!(
                        "{path}.{}: {table}.{field} must be {}{}",
                        contract.role,
                        if contract.nullable.contains(field) {
                            "nullable "
                        } else {
                            "required "
                        },
                        kind.sql()
                    )),
                }
            }
            if columns.get("id").is_none_or(|c| !c.pk || !c.auto) {
                errors.push(format!(
                    "{path}: {table}.id must be an auto integer primary key"
                ));
            }
            if contract.managed {
                for (field, col) in columns {
                    if !contract.fields.iter().any(|(f, _)| f == field)
                        && col.required
                        && col.default.is_none()
                    {
                        errors.push(format!(
                            "{path}: additional managed field {table}.{field} requires a default"
                        ));
                    }
                }
                for action in ["create", "update", "delete"] {
                    if schema
                        .operations
                        .get(table)
                        .and_then(|o| o.get(action))
                        .is_none_or(|o| o.enabled)
                    {
                        errors.push(format!("{path}: generic {table}.{action} must be disabled"));
                    }
                }
                for (action, operation) in schema.operations.get(table).into_iter().flatten() {
                    if action != "read" && operation.enabled {
                        errors.push(format!(
                            "{path}: {table}.{action} bypasses managed business operations"
                        ));
                    }
                }
                let read = schema.operations.get(table).and_then(|o| o.get("read"));
                if let Some(scope) = contract.read_scope {
                    if read.is_none_or(|op| {
                        !op.enabled
                            || op.public
                            || op.scope.as_deref() != Some(scope)
                            || op.relationships.len() != 1
                            || op.relationships[0].field.field != "user_id"
                    }) {
                        errors.push(format!(
                            "{path}: {table}.read must require {scope} and the user_id relationship"
                        ));
                    }
                } else if read.is_none_or(|op| op.enabled) {
                    errors.push(format!("{path}: internal {table}.read must be disabled"));
                }
            }
        }
    }
    errors.extend(crate::backoffice::validate(schema));
    if let Some(table) = entity(schema, "downloads", "file")
        && schema
            .tables
            .get(table)
            .and_then(|t| t.get("content_base64"))
            .is_some_and(|c| c.metadata["privacy"] != "secret")
    {
        errors
            .push("capabilities.downloads: file content must have secret privacy metadata".into());
    }
    let mut fk = |cap: &str, role: &str, field: &str, target_cap: &str, target_role: &str| {
        if let (Some(table), Some(target)) = (
            entity(schema, cap, role),
            entity(schema, target_cap, target_role),
        ) && schema
            .tables
            .get(table)
            .and_then(|t| t.get(field))
            .and_then(|c| c.references.as_deref())
            != Some(format!("{target}(id)").as_str())
        {
            errors.push(format!(
                "capabilities.{cap}: {table}.{field} must reference {target}(id)"
            ));
        }
    };
    for (cap, role) in [
        ("orders", "order"),
        ("payments", "payment"),
        ("subscriptions", "subscription"),
        ("entitlements", "entitlement"),
        ("accounts", "credential"),
        ("accounts", "staff"),
        ("customers", "customer"),
        ("password_auth", "credential"),
        ("password_auth", "session"),
    ] {
        fk(cap, role, "user_id", "accounts", "account");
    }
    for (cap, role, field, target, target_role) in [
        (
            "payments",
            "allocation",
            "transaction_id",
            "payments",
            "transaction",
        ),
        (
            "payments",
            "allocation",
            "payment_id",
            "payments",
            "payment",
        ),
        ("catalog", "price", "product_id", "catalog", "product"),
        ("orders", "item", "order_id", "orders", "order"),
        ("orders", "item", "price_id", "catalog", "price"),
        ("orders", "item", "product_id", "catalog", "product"),
        ("payments", "event", "payment_id", "payments", "payment"),
        (
            "subscriptions",
            "subscription",
            "price_id",
            "catalog",
            "price",
        ),
        (
            "subscriptions",
            "attempt",
            "subscription_id",
            "subscriptions",
            "subscription",
        ),
        (
            "subscriptions",
            "attempt",
            "payment_id",
            "payments",
            "payment",
        ),
        ("order_payments", "link", "order_id", "orders", "order"),
        (
            "order_payments",
            "link",
            "payment_id",
            "payments",
            "payment",
        ),
    ] {
        fk(cap, role, field, target, target_role);
    }
    for (cap, role, fields) in [
        ("catalog", "product", vec!["code"]),
        ("customers", "customer", vec!["user_id"]),
        ("accounts", "staff", vec!["user_id"]),
        ("password_auth", "credential", vec!["user_id"]),
        ("password_auth", "session", vec!["token_hash"]),
        ("orders", "order", vec!["user_id", "request_key"]),
        ("payments", "payment", vec!["user_id", "request_key"]),
        ("payments", "payment", vec!["reference"]),
        ("payments", "event", vec!["event_key"]),
        (
            "payments",
            "transaction",
            vec!["source", "account", "transaction_id"],
        ),
        (
            "payments",
            "allocation",
            vec!["transaction_id", "payment_id"],
        ),
        (
            "subscriptions",
            "subscription",
            vec!["user_id", "request_key"],
        ),
        ("subscriptions", "attempt", vec!["payment_id"]),
        (
            "subscriptions",
            "attempt",
            vec!["subscription_id", "request_key"],
        ),
        (
            "subscriptions",
            "attempt",
            vec!["subscription_id", "cycle", "attempt"],
        ),
        ("order_payments", "link", vec!["payment_id"]),
        ("order_payments", "link", vec!["order_id", "request_key"]),
        ("entitlements", "entitlement", vec!["source_key"]),
        ("downloads", "file", vec!["source_key"]),
    ] {
        if let Some(table) = entity(schema, cap, role)
            && !unique(schema, table, &fields)
        {
            errors.push(format!(
                "capabilities.{cap}: {table} needs database uniqueness on {}",
                fields.join(", ")
            ));
        }
    }
    for (cap, role, field, values) in [
        (
            "downloads",
            "file",
            "status",
            vec!["published", "withdrawn"],
        ),
        (
            "catalog",
            "price",
            "billing_interval",
            vec!["one_off", "month", "year"],
        ),
        (
            "orders",
            "order",
            "status",
            vec!["pending", "paid", "cancelled", "refunded"],
        ),
        (
            "payments",
            "payment",
            "status",
            vec!["pending", "succeeded", "failed", "cancelled", "refunded"],
        ),
        (
            "payments",
            "payment",
            "provider",
            vec!["bank_transfer", "fake"],
        ),
        (
            "payments",
            "transaction",
            "source",
            vec!["simulated", "starling", "open_banking"],
        ),
        ("payments", "transaction", "direction", vec!["in", "out"]),
        (
            "payments",
            "transaction",
            "status",
            vec!["pending", "settled", "reversed"],
        ),
        (
            "payments",
            "transaction",
            "disposition",
            vec!["open", "excluded"],
        ),
        ("payments", "allocation", "kind", vec!["receipt", "refund"]),
        (
            "payments",
            "event",
            "outcome",
            vec!["succeeded", "failed", "refunded"],
        ),
        (
            "subscriptions",
            "subscription",
            "status",
            vec!["pending", "active", "past_due", "cancelled"],
        ),
        (
            "subscriptions",
            "subscription",
            "billing_interval",
            vec!["month", "year"],
        ),
        (
            "entitlements",
            "entitlement",
            "status",
            vec!["active", "revoked"],
        ),
    ] {
        if let Some(table) = entity(schema, cap, role)
            && schema
                .tables
                .get(table)
                .and_then(|t| t.get(field))
                .is_some_and(|c| {
                    c.constraints
                        .values
                        .iter()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>()
                        != values.iter().copied().collect()
                })
        {
            errors.push(format!(
                "capabilities.{cap}: {table}.{field} does not match the declared state contract"
            ));
        }
    }
    if let Some(customer) = entity(schema, "accounts", "account")
        && customer != "user"
    {
        errors.push("capabilities.accounts: the platform identity contract requires user".into());
    }
    if let Some(credential) = entity(schema, "accounts", "credential")
        && credential != "api_key"
    {
        errors.push(
            "capabilities.accounts: the platform credential contract requires api_key".into(),
        );
    }
    errors
}

pub struct Endpoint {
    pub capability: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub scope: Option<&'static str>,
    pub customer: bool,
    pub input: Option<Value>,
}
fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
pub fn endpoints(schema: &Schema) -> Vec<Endpoint> {
    let key = json!({"type":"string","minLength":1,"maxLength":100});
    let id = json!({"type":"integer","minimum":1});
    let mut result = Vec::new();
    let mut add = |capability, method, path, scope, customer, input| {
        if schema.capabilities.contains_key(capability) {
            result.push(Endpoint {
                capability,
                method,
                path,
                scope,
                customer,
                input,
            });
        }
    };
    add("catalog", "get", "/b2c/catalog", None, false, None);
    add(
        "downloads",
        "get",
        "/b2c/downloads/manage",
        Some("downloads.publish"),
        false,
        None,
    );
    add(
        "downloads",
        "get",
        "/b2c/downloads",
        Some("downloads.read"),
        true,
        None,
    );
    add(
        "downloads",
        "get",
        "/b2c/downloads/{id}/content",
        Some("downloads.read"),
        true,
        None,
    );
    add(
        "downloads",
        "post",
        "/b2c/downloads",
        Some("downloads.publish"),
        false,
        Some(object(
            json!({"idempotency_key":key,"name":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_ -][A-Za-z0-9._ -]*$"},"media_type":{"type":"string","maxLength":100},"content_base64":{"type":"string","contentEncoding":"base64","maxLength":11184812},"resources":{"type":"array","minItems":1,"maxItems":20,"items":key}}),
            &[
                "idempotency_key",
                "name",
                "media_type",
                "content_base64",
                "resources",
            ],
        )),
    );
    add(
        "downloads",
        "post",
        "/b2c/downloads/{id}/withdraw",
        Some("downloads.publish"),
        false,
        Some(object(json!({}), &[])),
    );
    let password = json!({"type":"string","minLength":15,"maxLength":128,"writeOnly":true});
    add(
        "customers",
        "post",
        "/auth/signup",
        None,
        false,
        Some(object(
            json!({"email":{"type":"string","format":"email","maxLength":254},"password":password,"name":{"type":"string","minLength":1,"maxLength":120}}),
            &["email", "password"],
        )),
    );
    add(
        "password_auth",
        "post",
        "/auth/login",
        None,
        false,
        Some(object(
            json!({"email":{"type":"string","format":"email","maxLength":254},"password":{"type":"string","maxLength":128,"writeOnly":true}}),
            &["email", "password"],
        )),
    );
    add(
        "password_auth",
        "post",
        "/auth/logout",
        Some("auth.session"),
        false,
        Some(object(json!({}), &[])),
    );
    add(
        "password_auth",
        "post",
        "/auth/password",
        Some("auth.session"),
        false,
        Some(object(
            json!({"current_password":{"type":"string","maxLength":128,"writeOnly":true},"password":password}),
            &["current_password", "password"],
        )),
    );
    add(
        "orders",
        "post",
        "/b2c/orders",
        Some("orders.create"),
        true,
        Some(object(
            json!({"idempotency_key":key,"items":{"type":"array","minItems":1,"maxItems":20,"items":object(json!({"price_id":id,"quantity":{"type":"integer","minimum":1,"maximum":100}}), &["price_id","quantity"])}}),
            &["idempotency_key", "items"],
        )),
    );
    add(
        "orders",
        "get",
        "/b2c/orders/{id}",
        Some("orders.read"),
        true,
        None,
    );
    add(
        "orders",
        "post",
        "/b2c/orders/{id}/cancel",
        Some("orders.cancel"),
        true,
        Some(object(json!({}), &[])),
    );
    add(
        "order_payments",
        "post",
        "/b2c/orders/{id}/payments",
        Some("orders.pay"),
        true,
        Some(object(json!({"idempotency_key":key}), &["idempotency_key"])),
    );
    add(
        "payments",
        "post",
        "/b2c/payments",
        Some("payments.create"),
        true,
        Some(object(
            json!({"idempotency_key":key,"amount_minor":{"type":"integer","minimum":1},"currency":{"type":"string","pattern":"^[A-Z]{3}$"}}),
            &["idempotency_key", "amount_minor", "currency"],
        )),
    );
    add(
        "payments",
        "get",
        "/b2c/payments/pending",
        Some("payments.reconcile"),
        false,
        None,
    );
    add(
        "payments",
        "get",
        "/b2c/payments/{id}",
        Some("payments.read"),
        true,
        None,
    );
    add(
        "payments",
        "post",
        "/backoffice/transfers",
        Some("payments.import"),
        false,
        Some(object(
            json!({"transaction_id":{"type":"string","minLength":1,"maxLength":210},"account":{"type":"string"},"amount_minor":{"type":"integer","minimum":1},"currency":{"type":"string","const":"GBP"},"direction":{"enum":["in","out"]},"status":{"enum":["pending","settled","reversed"]},"reference":{"type":"string","maxLength":500},"bank_source":{"type":"string","maxLength":100},"source_reference":{"type":"string","maxLength":210},"narrative":{"type":"string","maxLength":2000},"settled_at":{"type":["string","null"],"format":"date-time"},"updated_at":{"type":"string","format":"date-time"}}),
            &[
                "transaction_id",
                "account",
                "amount_minor",
                "currency",
                "direction",
                "status",
                "updated_at",
            ],
        )),
    );
    add(
        "payments",
        "get",
        "/backoffice/transfers",
        Some("payments.reconcile"),
        false,
        None,
    );
    add(
        "payments",
        "post",
        "/backoffice/transfers/{id}/reconcile",
        Some("payments.reconcile"),
        false,
        Some(object(json!({}), &[])),
    );
    add(
        "payments",
        "post",
        "/backoffice/transfers/{id}/allocate",
        Some("payments.reconcile"),
        false,
        Some(object(
            json!({"payment_id":id,"amount_minor":{"type":"integer","minimum":1}}),
            &["payment_id", "amount_minor"],
        )),
    );
    add(
        "payments",
        "post",
        "/backoffice/transfers/sync",
        Some("payments.import"),
        false,
        Some(object(
            json!({"changes_since":{"type":"string","format":"date-time"}}),
            &["changes_since"],
        )),
    );
    if schema
        .capabilities
        .get("payments")
        .is_some_and(|c| c.settings["feed"] == "starling")
    {
        add(
            "payments",
            "post",
            "/backoffice/transfers/feed-item",
            None,
            false,
            None,
        );
    }
    add(
        "subscriptions",
        "post",
        "/b2c/subscriptions",
        Some("subscriptions.create"),
        true,
        Some(object(
            json!({"idempotency_key":key,"price_id":id}),
            &["idempotency_key", "price_id"],
        )),
    );
    add(
        "subscriptions",
        "get",
        "/b2c/subscriptions/{id}",
        Some("subscriptions.read"),
        true,
        None,
    );
    add(
        "subscriptions",
        "post",
        "/b2c/subscriptions/{id}/cancel",
        Some("subscriptions.cancel"),
        true,
        Some(object(
            json!({"at_period_end":{"type":"boolean","default":true}}),
            &[],
        )),
    );
    add(
        "subscriptions",
        "post",
        "/b2c/subscriptions/renew-due",
        Some("subscriptions.renew"),
        false,
        Some(object(json!({}), &[])),
    );
    add(
        "entitlements",
        "get",
        "/b2c/entitlements",
        Some("entitlements.read"),
        true,
        None,
    );
    add(
        "entitlements",
        "post",
        "/b2c/entitlements",
        Some("entitlements.grant"),
        false,
        Some(object(
            json!({"idempotency_key":key,"user_id":id,"resource":{"type":"string","minLength":1,"maxLength":100},"ends_at":{"type":["string","null"],"format":"date-time"}}),
            &["idempotency_key", "user_id", "resource"],
        )),
    );
    add(
        "entitlements",
        "post",
        "/b2c/entitlements/{id}/revoke",
        Some("entitlements.revoke"),
        false,
        Some(object(json!({}), &[])),
    );
    result.extend(crate::backoffice::endpoints(schema));
    if schema.capabilities.contains_key("payments") {
        result.push(Endpoint{capability:"payments",method:"post",path:"/backoffice/transfers/{id}/review",scope:Some("payments.reconcile"),customer:false,input:Some(object(json!({"exclude":{"type":"boolean"},"reason":{"type":"string","minLength":15,"maxLength":500}}),&["exclude","reason"]))});
    }
    let bank_feed = schema
        .capabilities
        .get("payments")
        .is_some_and(|c| c.settings["feed"] == "starling" || c.settings["feed"] == "open_banking");
    result.retain(|e| {
        !(bank_feed && e.path == "/backoffice/transfers" && e.method == "post"
            || !bank_feed && e.path == "/backoffice/transfers/sync")
    });
    result
}

pub fn customer_scopes(schema: &Schema) -> Vec<String> {
    endpoints(schema)
        .into_iter()
        .filter(|e| e.customer)
        .filter_map(|e| e.scope.map(str::to_string))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn add_openapi(schema: &Schema, doc: &mut Value) {
    doc["x-openb2c-capabilities"] = json!(schema.capabilities);
    if let Some(table) = entity(schema, "payments", "payment") {
        let mut request = doc["components"]["schemas"][crate::contract::pascal(table)].clone();
        request["properties"]["instructions"] = object(
            json!({"method":{"const":"bank_transfer"},"account_name":{"type":"string"},"sort_code":{"type":"string"},"account_number":{"type":"string"},"reference":{"type":"string"},"amount_minor":{"type":"integer"},"currency":{"const":"GBP"},"simulated":{"type":"boolean"},"qr_payload":{"type":"string","description":"Readable transfer instructions, not a banking-app payment URI"},"qr_svg":{"type":"string"}}),
            &[
                "method",
                "account_name",
                "sort_code",
                "account_number",
                "reference",
                "amount_minor",
                "currency",
                "simulated",
                "qr_payload",
                "qr_svg",
            ],
        );
        doc["components"]["schemas"]["OpenB2CPaymentRequest"] = request;
        let table = entity(schema, "payments", "transaction").unwrap();
        let mut transfer = doc["components"]["schemas"][crate::contract::pascal(table)].clone();
        transfer["properties"]["unallocated_minor"] = json!({"type":"integer","minimum":0});
        transfer["properties"]["allocations"] = json!({"type":"array","items":{"$ref":format!("#/components/schemas/{}",crate::contract::pascal(entity(schema,"payments","allocation").unwrap()))}});
        doc["components"]["schemas"]["OpenB2CBankTransfer"] = transfer;
        if schema
            .capabilities
            .get("payments")
            .is_some_and(|c| c.settings["feed"] == "starling")
        {
            doc["components"]["securitySchemes"]["starlingSignature"] = json!({"type":"apiKey","in":"header","name":"X-Hook-Signature","description":"Base64 RSA PKCS#1 v1.5 SHA-512 signature over the raw request body"});
        }
    }
    for endpoint in endpoints(schema) {
        let mut entry = json!({"tags":[endpoint.capability],"operationId":endpoint.path.trim_matches('/').replace(['/', '{', '}', '-'], "_"),"x-openb2c-scope":endpoint.scope,"responses":{"200":{"description":"Successful capability operation","content":{"application/json":{"schema":response_schema(schema, &endpoint)}}}}});
        if endpoint.path == "/auth/signup" {
            let response = entry["responses"]
                .as_object_mut()
                .unwrap()
                .remove("200")
                .unwrap();
            entry["responses"]["201"] = response;
        }
        if endpoint.path == "/b2c/downloads/{id}/content" {
            entry["responses"]["200"]["content"] =
                json!({"application/octet-stream":{"schema":{"type":"string","format":"binary"}}});
            entry["responses"]["200"]["description"] = json!(
                "File bytes with the stored media type, attachment filename, SHA-256 ETag and Cache-Control: private, no-store"
            );
        }
        if endpoint.capability == "password_auth" || endpoint.path == "/auth/signup" {
            entry["responses"]["429"] = json!({"description":"Authentication rate limit exceeded"});
        }
        entry["operationId"] = json!(format!(
            "{}_{}",
            endpoint.method,
            entry["operationId"].as_str().unwrap()
        ));
        for (code, description) in [
            ("401", "Authentication required"),
            ("403", "Insufficient scope or ownership"),
            ("404", "Capability or record not found"),
            ("409", "State or idempotency conflict"),
            ("422", "Invalid input"),
        ] {
            entry["responses"][code] = json!({"description":description});
        }
        if endpoint.scope.is_some() {
            entry["security"] = json!([{"bearerAuth":[]}]);
        }
        if endpoint.path.ends_with("/feed-item") {
            entry["security"] = json!([{"starlingSignature":[]}]);
        }
        if endpoint.path.contains("{id}") {
            entry["parameters"] = json!([{"name":"id","in":"path","required":true,"schema":{"type":"integer","minimum":1}}]);
        }
        if let Some(input) = endpoint.input {
            entry["requestBody"] = json!({"required":input["required"].as_array().is_some_and(|r|!r.is_empty()),"content":{"application/json":{"schema":input}}});
        }
        if doc["paths"].get(endpoint.path).is_none() {
            doc["paths"][endpoint.path] = json!({});
        }
        doc["paths"][endpoint.path][endpoint.method] = entry;
    }
}

fn response_schema(schema: &Schema, endpoint: &Endpoint) -> Value {
    let reference = |cap, role| json!({"$ref":format!("#/components/schemas/{}",if cap=="payments" && role=="payment"{"OpenB2CPaymentRequest".to_owned()}else{crate::contract::pascal(entity(schema,cap,role).unwrap())})});
    let array = |items| json!({"type":"array","items":items});
    match endpoint.capability {
        "payments" if endpoint.path.ends_with("/feed-item") => {
            object(json!({"accepted":{"type":"boolean"}}), &["accepted"])
        }
        "payments" if endpoint.path.starts_with("/backoffice/transfers") => {
            let transfer = json!({"$ref":"#/components/schemas/OpenB2CBankTransfer"});
            if endpoint.method == "get" || endpoint.path.ends_with("/sync") {
                let mut properties = json!({"items":array(transfer)});
                if endpoint.method == "get" {
                    properties["sync"] =
                        json!({"anyOf":[reference("payments","sync"),{"type":"null"}]});
                    properties["unresolved_count"] = json!({"type":"integer","minimum":0});
                }
                if endpoint.path.ends_with("/sync") {
                    properties["changes_since"] = json!({"type":"string","format":"date-time"});
                }
                object(properties, &["items"])
            } else {
                transfer
            }
        }
        "downloads" => {
            let file = object(
                json!({"id":{"type":"integer"},"name":{"type":"string"},"media_type":{"type":"string"},"sha256":{"type":"string"},"size_bytes":{"type":"integer"},"resources":{"type":"array","items":{"type":"string"}},"status":{"type":"string","enum":["published","withdrawn"]}}),
                &[
                    "id",
                    "name",
                    "media_type",
                    "sha256",
                    "size_bytes",
                    "resources",
                    "status",
                ],
            );
            if endpoint.method == "get" && !endpoint.path.ends_with("/content") {
                object(json!({"items":array(file)}), &["items"])
            } else {
                file
            }
        }
        "password_auth" if endpoint.path == "/auth/logout" => {
            object(json!({"revoked":{"type":"boolean"}}), &["revoked"])
        }
        "password_auth" | "customers" => object(
            json!({"access_token":{"type":"string"},"token_type":{"type":"string","const":"Bearer"},"expires_in":{"type":"integer","const":3600},"expires_at":{"type":"string","format":"date-time"},"user":{"type":"object","properties":{"id":{"type":"integer"},"email":{"type":"string"},"name":{"type":"string"}},"required":["id","email","name"]}}),
            &[
                "access_token",
                "token_type",
                "expires_in",
                "expires_at",
                "user",
            ],
        ),
        "catalog" => object(
            json!({"items":array(object(json!({"product":reference("catalog","product"),"prices":array(reference("catalog","price"))}), &["product","prices"]))}),
            &["items"],
        ),
        "orders" => object(
            json!({"order":reference("orders","order"),"items":array(reference("orders","item"))}),
            &["order", "items"],
        ),
        "payments" if endpoint.path.ends_with("pending") => object(
            json!({"items":array(reference("payments","payment"))}),
            &["items"],
        ),
        "payments" | "order_payments" => reference("payments", "payment"),
        "subscriptions" if endpoint.path.ends_with("renew-due") => object(
            json!({"payments":array(reference("payments","payment")),"cancelled":{"type":"integer"}}),
            &["payments", "cancelled"],
        ),
        "subscriptions" => object(
            json!({"subscription":reference("subscriptions","subscription"),"payment":{"anyOf":[reference("payments","payment"),{"type":"null"}]},"standing_order_notice":{"type":"string"}}),
            &["subscription", "payment"],
        ),
        "entitlements" if endpoint.method == "get" => object(
            json!({"items":array(reference("entitlements","entitlement"))}),
            &["items"],
        ),
        "entitlements" => reference("entitlements", "entitlement"),
        _ => crate::backoffice::response_schema(schema, endpoint),
    }
}
