//! REST adapter for explicit internal operations; every service method checks its own scope.
use super::*;
use axum::http::{HeaderMap, Method};
pub(crate) fn dispatch(
    s: &Service,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    bytes: &[u8],
    auth: &AuthContext,
) -> Result<Value> {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    let endpoint = openb2c_schema::capabilities::endpoints(&s.schema)
        .into_iter()
        .find(|e| {
            e.method.eq_ignore_ascii_case(method.as_str()) && {
                let pattern = e.path.trim_matches('/').split('/').collect::<Vec<_>>();
                pattern.len() == parts.len()
                    && pattern
                        .iter()
                        .zip(&parts)
                        .all(|(a, b)| *a == "{id}" || a == b)
            }
        })
        .ok_or_else(|| ServiceError::new("not_found", "Unknown backoffice endpoint"))?;
    if let Some(scope) = endpoint.scope {
        s.cap_command(endpoint.capability, scope, auth)?;
    }
    let input = if *method == Method::POST {
        crate::web::json_body(headers, bytes)?
    } else {
        json!({})
    };
    let id = || {
        parts
            .get(2)
            .and_then(|p| p.parse::<i64>().ok())
            .filter(|i| *i > 0)
            .ok_or_else(|| invalid("A positive record ID is required"))
    };
    match (method.as_str(), parts.as_slice()) {
        ("GET", ["backoffice", "accounts"]) => s.backoffice_list("accounting", "account", auth),
        ("POST", ["backoffice", "accounts"]) => s.backoffice_account(&input, auth),
        ("POST", ["backoffice", "journals"]) => s.backoffice_journal(&input, auth),
        ("GET", ["backoffice", "journals", _]) => s.backoffice_journal_view(id()?, auth),
        ("POST", ["backoffice", "journals", _, "reverse"]) => {
            s.backoffice_reverse(id()?, &input, auth)
        }
        ("POST", ["backoffice", "periods", "close"]) => s.backoffice_close(&input, auth),
        ("POST", ["backoffice", "reports", "financial"]) => {
            s.backoffice_financial_report(&input, auth)
        }
        ("GET", ["backoffice", "invoices"]) => s.backoffice_list("billing", "invoice", auth),
        ("GET", ["backoffice", "invoices", _]) => s.backoffice_invoice_view(id()?, auth),
        ("POST", ["backoffice", "invoices"]) => s.backoffice_invoice(&input, auth),
        ("POST", ["backoffice", "invoices", _, "receive"]) => {
            s.backoffice_receive(id()?, &input, auth)
        }
        ("POST", ["backoffice", "invoices", _, "refund"]) => {
            s.backoffice_refund(id()?, &input, auth)
        }
        ("GET", ["backoffice", "billing-events"]) => s.backoffice_list("billing", "event", auth),
        ("POST", ["backoffice", "billing-events", "post"]) => {
            s.backoffice_post_billing_events(&input, auth)
        }
        ("POST", ["backoffice", "invoices", _, "credit"]) => {
            s.backoffice_credit(id()?, &input, auth)
        }
        ("POST", ["backoffice", "revenue", "recognize"]) => s.backoffice_recognize(&input, auth),
        ("POST", ["backoffice", "documents"]) => s.backoffice_document(&input, auth),
        ("GET", ["backoffice", "documents", _]) => {
            s.backoffice_record("documents", "document", id()?, auth)
        }
        ("GET", ["backoffice", "suppliers"]) => s.backoffice_list("purchasing", "supplier", auth),
        ("POST", ["backoffice", "suppliers"]) => s.backoffice_supplier(&input, auth),
        ("GET", ["backoffice", "expenses"]) => s.backoffice_list("purchasing", "expense", auth),
        ("GET", ["backoffice", "expenses", _]) => {
            s.backoffice_record("purchasing", "expense", id()?, auth)
        }
        ("POST", ["backoffice", "expenses"]) => s.backoffice_expense(&input, auth),
        ("POST", ["backoffice", "expenses", _, "pay"]) => {
            s.backoffice_pay_expense(id()?, &input, auth)
        }
        ("POST", ["backoffice", "expenses", _, "void"]) => {
            s.backoffice_void_expense(id()?, &input, auth)
        }
        ("GET", ["backoffice", "bank-lines"]) => s.backoffice_list("banking", "statement", auth),
        ("POST", ["backoffice", "bank-lines"]) => s.backoffice_bank_line(&input, auth),
        ("POST", ["backoffice", "bank-lines", _, "match"]) => {
            s.backoffice_match(id()?, &input, auth)
        }
        ("POST", ["backoffice", "reports", "vat"]) => s.backoffice_vat_report(&input, auth),
        _ => Err(ServiceError::new(
            "not_found",
            "Unknown backoffice endpoint",
        )),
    }
}
