//! Explicit contracts for internal business capabilities and their REST operations.
use crate::{
    Schema, SqlType,
    capabilities::{Endpoint, EntityContract, entity},
};
use serde_json::{Value, json};

pub(crate) fn contracts(name: &str) -> Vec<EntityContract> {
    use SqlType::{Integer as I, Text as T};
    let table = |role, fields, nullable| EntityContract {
        role,
        fields,
        nullable,
        read_scope: None,
        managed: true,
    };
    match name {
        "accounting" => vec![
            table(
                "account",
                vec![("id", I), ("code", T), ("name", T), ("kind", T)],
                &[],
            ),
            table(
                "journal",
                vec![
                    ("id", I),
                    ("source_key", T),
                    ("request_json", T),
                    ("date", T),
                    ("currency", T),
                    ("description", T),
                    ("reverses_id", I),
                ],
                &["reverses_id"],
            ),
            table(
                "line",
                vec![
                    ("id", I),
                    ("journal_id", I),
                    ("account_id", I),
                    ("debit", I),
                    ("credit", I),
                ],
                &[],
            ),
            table(
                "period",
                vec![("id", I), ("through_date", T), ("created_at", T)],
                &[],
            ),
        ],
        "billing" => vec![
            table(
                "refund",
                vec![
                    ("id", I),
                    ("invoice_id", I),
                    ("date", T),
                    ("reason", T),
                    ("journal_id", I),
                ],
                &[],
            ),
            table(
                "event",
                vec![
                    ("id", I),
                    ("source_key", T),
                    ("payload_json", T),
                    ("occurred_at", T),
                    ("status", T),
                ],
                &[],
            ),
            table(
                "invoice",
                vec![
                    ("id", I),
                    ("source_key", T),
                    ("request_json", T),
                    ("issuer", T),
                    ("customer", T),
                    ("description", T),
                    ("date", T),
                    ("due_date", T),
                    ("posting_date", T),
                    ("currency", T),
                    ("net_minor", I),
                    ("tax_minor", I),
                    ("tax_code", T),
                    ("period_start", T),
                    ("period_end", T),
                    ("recognized_minor", I),
                    ("journal_id", I),
                ],
                &["period_start", "period_end", "posting_date"],
            ),
            table(
                "credit",
                vec![
                    ("id", I),
                    ("invoice_id", I),
                    ("date", T),
                    ("reason", T),
                    ("journal_id", I),
                ],
                &[],
            ),
            table(
                "receipt",
                vec![("id", I), ("invoice_id", I), ("date", T), ("journal_id", I)],
                &[],
            ),
        ],
        "documents" => vec![table(
            "document",
            vec![
                ("id", I),
                ("source_key", T),
                ("name", T),
                ("media_type", T),
                ("content_base64", T),
                ("sha256", T),
            ],
            &[],
        )],
        "purchasing" => vec![
            table(
                "supplier",
                vec![("id", I), ("source_key", T), ("name", T), ("email", T)],
                &[],
            ),
            table(
                "expense",
                vec![
                    ("id", I),
                    ("source_key", T),
                    ("request_json", T),
                    ("supplier_id", I),
                    ("supplier_name", T),
                    ("document_id", I),
                    ("supplier_reference", T),
                    ("posting_date", T),
                    ("description", T),
                    ("date", T),
                    ("currency", T),
                    ("net_minor", I),
                    ("tax_minor", I),
                    ("tax_code", T),
                    ("recoverable", I),
                    ("journal_id", I),
                    ("payment_journal_id", I),
                    ("void_journal_id", I),
                ],
                &["payment_journal_id", "void_journal_id", "posting_date"],
            ),
        ],
        "banking" => vec![table(
            "statement",
            vec![
                ("id", I),
                ("source_key", T),
                ("account_id", I),
                ("date", T),
                ("currency", T),
                ("amount_minor", I),
                ("description", T),
                ("journal_line_id", I),
            ],
            &["journal_line_id"],
        )],
        _ => vec![],
    }
}

pub(crate) fn validate(schema: &Schema) -> Vec<String> {
    let mut errors = vec![];
    if schema.capabilities.contains_key("accounting")
        && schema.metadata.get("currency").is_some_and(|c| c != "GBP")
    {
        errors.push("capabilities.accounting: UK ledger currency must be GBP".into());
    }
    for (cap, role, field, target, target_role) in [
        ("billing", "refund", "invoice_id", "billing", "invoice"),
        ("billing", "refund", "journal_id", "accounting", "journal"),
        ("accounting", "line", "journal_id", "accounting", "journal"),
        ("accounting", "line", "account_id", "accounting", "account"),
        (
            "accounting",
            "journal",
            "reverses_id",
            "accounting",
            "journal",
        ),
        ("billing", "invoice", "journal_id", "accounting", "journal"),
        ("billing", "credit", "invoice_id", "billing", "invoice"),
        ("billing", "receipt", "invoice_id", "billing", "invoice"),
        ("billing", "credit", "journal_id", "accounting", "journal"),
        ("billing", "receipt", "journal_id", "accounting", "journal"),
        (
            "purchasing",
            "expense",
            "supplier_id",
            "purchasing",
            "supplier",
        ),
        (
            "purchasing",
            "expense",
            "document_id",
            "documents",
            "document",
        ),
        (
            "purchasing",
            "expense",
            "journal_id",
            "accounting",
            "journal",
        ),
        (
            "purchasing",
            "expense",
            "payment_journal_id",
            "accounting",
            "journal",
        ),
        (
            "purchasing",
            "expense",
            "void_journal_id",
            "accounting",
            "journal",
        ),
        (
            "banking",
            "statement",
            "account_id",
            "accounting",
            "account",
        ),
        (
            "banking",
            "statement",
            "journal_line_id",
            "accounting",
            "line",
        ),
    ] {
        if let (Some(table), Some(target)) = (
            entity(schema, cap, role),
            entity(schema, target, target_role),
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
    }
    for (cap, role, field) in [
        ("billing", "refund", "invoice_id"),
        ("billing", "event", "source_key"),
        ("accounting", "account", "code"),
        ("accounting", "journal", "source_key"),
        ("accounting", "journal", "reverses_id"),
        ("accounting", "period", "through_date"),
        ("billing", "invoice", "source_key"),
        ("billing", "receipt", "invoice_id"),
        ("billing", "credit", "invoice_id"),
        ("documents", "document", "source_key"),
        ("purchasing", "supplier", "source_key"),
        ("purchasing", "expense", "source_key"),
        ("banking", "statement", "source_key"),
        ("banking", "statement", "journal_line_id"),
    ] {
        if let Some(table) = entity(schema, cap, role)
            && !super::capabilities::unique(schema, table, &[field])
        {
            errors.push(format!(
                "capabilities.{cap}: {table}.{field} needs database uniqueness"
            ));
        }
    }
    if let Some(t) = entity(schema, "purchasing", "expense")
        && !super::capabilities::unique(schema, t, &["supplier_id", "supplier_reference"])
    {
        errors.push(
            "capabilities.purchasing: supplier invoice reference needs database uniqueness".into(),
        );
    }
    if let Some(cap) = schema.capabilities.get("billing") {
        let code = cap.settings["defaultTaxCode"].as_str().unwrap_or("");
        if ![
            "unassessed",
            "standard",
            "reduced",
            "zero",
            "exempt",
            "outside",
            "not_registered",
        ]
        .contains(&code)
            || cap.settings.as_object().is_none_or(|s| s.len() != 1)
        {
            errors.push(
                "capabilities.billing.settings: expected defaultTaxCode UK VAT treatment".into(),
            );
        }
    }
    if let Some(cap) = schema.capabilities.get("tax")
        && (cap.settings["jurisdiction"] != "GB"
            || !["unknown", "registered", "not_registered"]
                .contains(&cap.settings["vatRegistration"].as_str().unwrap_or(""))
            || cap.settings.as_object().is_none_or(|s| s.len() != 2))
    {
        errors.push(
            "capabilities.tax.settings: expected GB jurisdiction and explicit vatRegistration"
                .into(),
        );
    }
    errors
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
pub(crate) fn endpoints(schema: &Schema) -> Vec<Endpoint> {
    let mut result = vec![];
    let mut add = |capability, method, path, scope, input| {
        if schema.capabilities.contains_key(capability) {
            result.push(Endpoint {
                capability,
                method,
                path,
                scope: Some(scope),
                customer: false,
                input,
            });
        }
    };
    let string = json!({"type":"string","minLength":1,"maxLength":500});
    let key = json!({"type":"string","minLength":1,"maxLength":100});
    let date = json!({"type":"string","format":"date"});
    let id = json!({"type":"integer","minimum":1});
    let money = json!({"type":"integer","minimum":0});
    let tax_code = json!({"type":"string","enum":["unassessed","standard","reduced","zero","exempt","outside","not_registered"]});
    let amount = json!({"net_minor":money,"tax_code":tax_code});
    add(
        "accounting",
        "get",
        "/backoffice/accounts",
        "accounting.read",
        None,
    );
    add(
        "accounting",
        "post",
        "/backoffice/accounts",
        "accounting.write",
        Some(object(
            json!({"code":key,"name":string,"kind":{"enum":["asset","liability","equity","income","expense"]}}),
            &["code", "name", "kind"],
        )),
    );
    add(
        "accounting",
        "post",
        "/backoffice/journals",
        "accounting.write",
        Some(object(
            json!({"idempotency_key":key,"date":date,"currency":{"const":"GBP"},"description":string,"lines":{"type":"array","minItems":2,"maxItems":100,"items":object(json!({"account":key,"debit":money,"credit":money}), &["account","debit","credit"])}}),
            &["idempotency_key", "date", "description", "lines"],
        )),
    );
    add(
        "accounting",
        "get",
        "/backoffice/journals/{id}",
        "accounting.read",
        None,
    );
    add(
        "accounting",
        "post",
        "/backoffice/journals/{id}/reverse",
        "accounting.write",
        Some(object(
            json!({"date":date,"reason":string}),
            &["date", "reason"],
        )),
    );
    add(
        "accounting",
        "post",
        "/backoffice/periods/close",
        "accounting.close",
        Some(object(json!({"through_date":date}), &["through_date"])),
    );
    let report = object(json!({"from":date,"through":date}), &["from", "through"]);
    add(
        "accounting",
        "post",
        "/backoffice/reports/financial",
        "accounting.read",
        Some(report.clone()),
    );
    add(
        "billing",
        "post",
        "/backoffice/invoices/{id}/refund",
        "billing.refund",
        Some(object(
            json!({"date":date,"reason":string}),
            &["date", "reason"],
        )),
    );
    add(
        "billing",
        "get",
        "/backoffice/billing-events",
        "billing.read",
        None,
    );
    add(
        "billing",
        "post",
        "/backoffice/billing-events/post",
        "billing.write",
        Some(object(json!({"date":date}), &["date"])),
    );
    add(
        "billing",
        "get",
        "/backoffice/invoices",
        "billing.read",
        None,
    );
    add(
        "billing",
        "get",
        "/backoffice/invoices/{id}",
        "billing.read",
        None,
    );
    let mut props = amount.as_object().unwrap().clone();
    props.extend(json!({"idempotency_key":key,"customer":string,"description":string,"date":date,"due_date":date,"period_start":date,"period_end":date,"posting_date":date}).as_object().unwrap().clone());
    add(
        "billing",
        "post",
        "/backoffice/invoices",
        "billing.write",
        Some(object(
            Value::Object(props),
            &[
                "idempotency_key",
                "customer",
                "description",
                "date",
                "due_date",
                "net_minor",
                "tax_code",
            ],
        )),
    );
    add(
        "billing",
        "post",
        "/backoffice/invoices/{id}/receive",
        "billing.write",
        Some(object(json!({"date":date}), &["date"])),
    );
    add(
        "billing",
        "post",
        "/backoffice/invoices/{id}/credit",
        "billing.write",
        Some(object(
            json!({"date":date,"reason":string}),
            &["date", "reason"],
        )),
    );
    add(
        "billing",
        "post",
        "/backoffice/revenue/recognize",
        "billing.write",
        Some(object(json!({"through_date":date}), &["through_date"])),
    );
    add(
        "documents",
        "post",
        "/backoffice/documents",
        "documents.write",
        Some(object(
            json!({"idempotency_key":key,"name":string,"media_type":string,"content_base64":{"type":"string","maxLength":500000}}),
            &["idempotency_key", "name", "media_type", "content_base64"],
        )),
    );
    add(
        "documents",
        "get",
        "/backoffice/documents/{id}",
        "documents.read",
        None,
    );
    add(
        "purchasing",
        "post",
        "/backoffice/suppliers",
        "purchasing.write",
        Some(object(
            json!({"idempotency_key":key,"name":string,"email":string}),
            &["idempotency_key", "name", "email"],
        )),
    );
    add(
        "purchasing",
        "get",
        "/backoffice/suppliers",
        "purchasing.read",
        None,
    );
    let mut props = amount.as_object().unwrap().clone();
    props.extend(json!({"idempotency_key":key,"supplier_id":id,"document_id":id,"supplier_reference":string,"description":string,"date":date,"posting_date":date,"recoverable":{"type":"boolean","default":false}}).as_object().unwrap().clone());
    add(
        "purchasing",
        "post",
        "/backoffice/expenses",
        "purchasing.write",
        Some(object(
            Value::Object(props),
            &[
                "idempotency_key",
                "supplier_id",
                "document_id",
                "supplier_reference",
                "description",
                "date",
                "net_minor",
                "tax_code",
            ],
        )),
    );
    add(
        "purchasing",
        "get",
        "/backoffice/expenses",
        "purchasing.read",
        None,
    );
    add(
        "purchasing",
        "get",
        "/backoffice/expenses/{id}",
        "purchasing.read",
        None,
    );
    add(
        "purchasing",
        "post",
        "/backoffice/expenses/{id}/pay",
        "purchasing.write",
        Some(object(json!({"date":date}), &["date"])),
    );
    add(
        "purchasing",
        "post",
        "/backoffice/expenses/{id}/void",
        "purchasing.write",
        Some(object(
            json!({"date":date,"reason":string}),
            &["date", "reason"],
        )),
    );
    add(
        "banking",
        "post",
        "/backoffice/bank-lines",
        "banking.write",
        Some(object(
            json!({"idempotency_key":key,"account":key,"date":date,"amount_minor":{"type":"integer"},"description":string}),
            &[
                "idempotency_key",
                "account",
                "date",
                "amount_minor",
                "description",
            ],
        )),
    );
    add(
        "banking",
        "get",
        "/backoffice/bank-lines",
        "banking.read",
        None,
    );
    add(
        "banking",
        "post",
        "/backoffice/bank-lines/{id}/match",
        "banking.write",
        Some(object(json!({"journal_line_id":id}), &["journal_line_id"])),
    );
    add(
        "tax",
        "post",
        "/backoffice/reports/vat",
        "tax.read",
        Some(report),
    );
    result
}

pub(crate) fn response_schema(schema: &Schema, endpoint: &Endpoint) -> Value {
    let record = |cap, role| {
        let fields = &schema.tables[entity(schema, cap, role).unwrap()];
        let properties = fields
            .iter()
            .map(|(f, c)| (f.clone(), crate::contract::field_schema(f, c, true)))
            .collect::<serde_json::Map<_, _>>();
        let required = fields
            .iter()
            .filter(|(_, c)| c.required || c.pk)
            .map(|(f, _)| f.as_str())
            .collect::<Vec<_>>();
        object(Value::Object(properties), &required)
    };

    let reference = record;
    let number = json!({"type":"integer"});
    if endpoint.capability == "documents" {
        return record("documents", "document");
    }
    if endpoint.path == "/backoffice/reports/financial" {
        return object(
            json!({"currency":{"const":"GBP"},"from":{"type":"string","format":"date"},"through":{"type":"string","format":"date"},"balanced":{"type":"boolean"},"unposted_billing_events":{"type":"integer"},"unresolved_bank_transactions":{"type":"integer"},"trial_balance":{"type":"array","items":{"type":"object","properties":{"id":number,"code":{"type":"string"},"name":{"type":"string"},"kind":{"type":"string"},"balance_minor":number,"period_movement_minor":number}}},"profit_and_loss":object(json!({"income_minor":number,"expenses_minor":number,"profit_minor":number}),&["income_minor","expenses_minor","profit_minor"]),"balance_sheet":object(json!({"assets_minor":number,"liabilities_minor":number,"equity_minor":number,"accumulated_earnings_minor":number}),&["assets_minor","liabilities_minor","equity_minor","accumulated_earnings_minor"])}),
            &[
                "currency",
                "from",
                "through",
                "balanced",
                "trial_balance",
                "profit_and_loss",
                "balance_sheet",
            ],
        );
    }
    if endpoint.capability == "tax" {
        return object(
            json!({"jurisdiction":{"const":"GB"},"rules_version":{"const":1},"scheme":{"const":"standard_accrual_domestic"},"currency":{"const":"GBP"},"from":{"type":"string","format":"date"},"through":{"type":"string","format":"date"},"draft":{"const":true},"requires_review":{"const":true},"filing_supported":{"const":false},"blockers":{"type":"array","items":{"type":"object"}},"evidence":{"type":"array","items":{"type":"object"}},"boxes":object(json!({"1_vat_due_sales_minor":number,"2_vat_due_acquisitions_minor":number,"3_total_vat_due_minor":number,"4_vat_reclaimed_minor":number,"5_net_vat_minor":number,"6_sales_whole_pounds":number,"7_purchases_whole_pounds":number,"8_dispatches_whole_pounds":number,"9_acquisitions_whole_pounds":number}),&["1_vat_due_sales_minor","2_vat_due_acquisitions_minor","3_total_vat_due_minor","4_vat_reclaimed_minor","5_net_vat_minor","6_sales_whole_pounds","7_purchases_whole_pounds","8_dispatches_whole_pounds","9_acquisitions_whole_pounds"]),"net_position":{"enum":["payable","repayable"]},"scope":{"type":"string"}}),
            &[
                "jurisdiction",
                "rules_version",
                "scheme",
                "currency",
                "from",
                "through",
                "draft",
                "requires_review",
                "filing_supported",
                "blockers",
                "evidence",
                "boxes",
                "net_position",
                "scope",
            ],
        );
    }
    if endpoint.capability == "accounting" && endpoint.path.contains("journals") {
        let mut line = record("accounting", "line");
        line["properties"]["account"] = json!({"type":"string"});
        return if endpoint.path.ends_with("reverse") {
            reference("accounting", "journal")
        } else {
            object(
                json!({"journal":reference("accounting","journal"),"lines":{"type":"array","items":line}}),
                &["journal", "lines"],
            )
        };
    }
    if endpoint.capability == "accounting" && endpoint.path.ends_with("close") {
        return reference("accounting", "period");
    }

    if endpoint.path.ends_with("billing-events/post") {
        return object(json!({"posted":{"type":"integer"}}), &["posted"]);
    }
    let role = match endpoint.capability {
        "billing" if endpoint.path.ends_with("billing-events") => Some(("billing", "event")),
        "billing" if endpoint.path.ends_with("recognize") => {
            return object(
                json!({"recognized_minor":{"type":"integer"}}),
                &["recognized_minor"],
            );
        }
        "billing" => Some(("billing", "invoice")),
        "documents" => Some(("documents", "document")),
        "purchasing" if endpoint.path.contains("suppliers") => Some(("purchasing", "supplier")),
        "purchasing" => Some(("purchasing", "expense")),
        "banking" => Some(("banking", "statement")),
        "accounting" if endpoint.path.ends_with("accounts") => Some(("accounting", "account")),
        _ => None,
    };
    if let Some((cap, role)) = role {
        let mut entity_schema = record(cap, role);
        if cap == "billing" && endpoint.method != "post" && endpoint.path.contains("{id}")
            || cap == "billing"
                && (endpoint.path.ends_with("receive")
                    || endpoint.path.ends_with("credit")
                    || endpoint.path.ends_with("refund"))
        {
            entity_schema["properties"]["refund"] =
                json!({"anyOf":[record("billing","refund"),{"type":"null"}]});
            entity_schema["properties"]["refund_due_minor"] = json!({"type":"integer"});
            entity_schema["properties"]["number"] = json!({"type":"string"});
            entity_schema["properties"]["outstanding_minor"] = json!({"type":"integer"});
            entity_schema["properties"]["credit_note"] =
                json!({"anyOf":[record("billing","credit"),{"type":"null"}]});
            entity_schema["properties"]["receipt"] =
                json!({"anyOf":[record("billing","receipt"),{"type":"null"}]});
        }
        if endpoint.method == "get" && !endpoint.path.contains("{id}") {
            return object(
                json!({"items":{"type":"array","items":entity_schema}}),
                &["items"],
            );
        }
        return entity_schema;
    }
    json!({"type":"object"})
}
