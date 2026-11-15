//! HTTP routing only. Every capability service repeats its own authorization.
use super::*;
use axum::http::{HeaderMap, Method};

pub(crate) fn dispatch(
    service: &Service,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    bytes: &[u8],
    auth: &AuthContext,
) -> Result<Value> {
    let segments = path.trim_matches('/').split('/').collect::<Vec<_>>();
    let endpoint = openb2c_schema::capabilities::endpoints(&service.schema)
        .into_iter()
        .filter(|e| {
            e.method.eq_ignore_ascii_case(method.as_str()) && {
                let template = e.path.trim_matches('/').split('/').collect::<Vec<_>>();
                template.len() == segments.len()
                    && template
                        .iter()
                        .zip(&segments)
                        .all(|(a, b)| *a == "{id}" || a == b)
            }
        })
        .min_by_key(|e| e.path.matches("{id}").count())
        .ok_or_else(|| ServiceError::new("not_found", "Unknown capability endpoint"))?;
    if let Some(scope) = endpoint.scope {
        service.cap_command(endpoint.capability, scope, auth)?;
    }
    let input = if method == Method::POST && !bytes.is_empty() {
        crate::web::json_body(headers, bytes)?
    } else {
        json!({})
    };
    let id = || {
        segments
            .get(2)
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| invalid("A positive record ID is required"))
    };
    match (method.as_str(), segments.as_slice()) {
        ("GET", ["b2c", "catalog"]) => service.b2c_catalog(),
        ("POST", ["b2c", "orders"]) => service.b2c_create_order(&input, auth),
        ("GET", ["b2c", "orders", _]) => service.b2c_order(id()?, auth),
        ("POST", ["b2c", "orders", _, "cancel"]) => {
            decode::<Empty>(&input)?;
            service.b2c_cancel_order(id()?, auth)
        }
        ("POST", ["b2c", "orders", _, "payments"]) => {
            service.b2c_order_payment(id()?, &input, auth)
        }
        ("POST", ["b2c", "payments"]) => service.b2c_create_payment(&input, auth),
        ("GET", ["b2c", "payments", "pending"]) => service.b2c_pending_payments(auth),
        ("GET", ["b2c", "payments", _]) => service.b2c_payment(id()?, auth),
        ("POST", ["b2c", "payments", _, "simulate"]) => {
            service.b2c_simulate_payment(id()?, &input, auth)
        }
        ("POST", ["b2c", "subscriptions"]) => service.b2c_create_subscription(&input, auth),
        ("POST", ["b2c", "subscriptions", "renew-due"]) => {
            decode::<Empty>(&input)?;
            service.b2c_renew_due(auth)
        }
        ("GET", ["b2c", "subscriptions", _]) => service.b2c_subscription(id()?, auth),
        ("POST", ["b2c", "subscriptions", _, "cancel"]) => {
            service.b2c_cancel_subscription(id()?, &input, auth)
        }
        ("POST", ["b2c", "subscriptions", _, "retry"]) => {
            service.b2c_retry_subscription(id()?, &input, auth)
        }
        ("GET", ["b2c", "entitlements"]) => service.b2c_entitlements(auth),
        ("POST", ["b2c", "entitlements"]) => service.b2c_grant_entitlement(&input, auth),
        ("POST", ["b2c", "entitlements", _, "revoke"]) => {
            decode::<Empty>(&input)?;
            service.b2c_revoke_entitlement(id()?, auth)
        }
        _ => Err(ServiceError::new(
            "not_found",
            "Unknown capability endpoint",
        )),
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
