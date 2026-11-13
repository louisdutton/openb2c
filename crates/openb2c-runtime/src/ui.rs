use crate::{
    AuthContext, Service,
    commerce::label,
    identity::{fake_allowed, production, secret},
    service::{ListOptions, Result, ServiceError},
    web::{AppState, cookie, header},
};
use axum::{
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{Html, IntoResponse, Response},
};
use openb2c_schema::{Column, SqlType, inference};
use rusqlite::{OptionalExtension, params};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn slug(s: &str) -> String {
    let value = s
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if value.is_empty() {
        "openb2c".into()
    } else {
        value
    }
}
pub fn title(s: &str) -> String {
    s.split('_')
        .map(|s| {
            let mut chars = s.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn display(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| {
        if v.is_null() {
            String::new()
        } else {
            v.to_string()
        }
    })
}
fn money(amount: i64) -> String {
    format!("£{}.{:02}", amount / 100, amount.abs() % 100)
}
fn timestamp(s: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.format("%d %b %Y, %H:%M").to_string())
        .unwrap_or_else(|_| s.into())
}
fn text(field: &str, value: &Value, col: Option<&Column>) -> String {
    if field.ends_with("_pence") || col.is_some_and(|c| c.metadata["format"] == "money") {
        return value.as_i64().map(money).unwrap_or_default();
    }
    if field.ends_with("_at") {
        return timestamp(value.as_str().unwrap_or(""));
    }
    if field.ends_with("_mins") || col.is_some_and(|c| c.metadata["format"] == "duration-minutes") {
        return value
            .as_i64()
            .map(|v| {
                format!(
                    "{}h{}",
                    v / 60,
                    if v % 60 > 0 {
                        format!(" {}m", v % 60)
                    } else {
                        String::new()
                    }
                )
            })
            .unwrap_or_default();
    }
    display(value)
}
fn private(col: &Column) -> bool {
    col.metadata["redact"] == true
        || ["secret", "internal", "sensitive"]
            .iter()
            .any(|v| col.metadata["privacy"] == *v)
}
fn cookie_header(name: &str, value: &str, max_age: i64) -> String {
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if production() { "; Secure" } else { "" }
    )
}
fn set_cookie(response: &mut Response, name: &str, value: &str, age: i64) {
    if let Ok(v) = HeaderValue::from_str(&cookie_header(name, value, age)) {
        response.headers_mut().append("set-cookie", v);
    }
}
fn redirect(path: &str) -> Response {
    let safe = if path.starts_with('/') && !path.starts_with("//") && !path.contains(['\r', '\n']) {
        path
    } else {
        "/"
    };
    (StatusCode::SEE_OTHER, [("location", safe)]).into_response()
}
fn form_start(action: &str, csrf: &str) -> String {
    format!(
        "<form method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"_csrf\" value=\"{}\">",
        escape(action),
        escape(csrf)
    )
}
fn require_user(auth: &AuthContext) -> Result<()> {
    if auth.user_id.is_some() || auth.has_scope("*") {
        Ok(())
    } else {
        Err(ServiceError::new("unauthorized", "Sign in to continue"))
    }
}
fn shell(
    service: &Service,
    auth: &AuthContext,
    title_text: &str,
    content: &str,
    csrf: &str,
) -> Response {
    let name = if service.schema.organization.name.is_empty() {
        "OpenB2C"
    } else {
        &service.schema.organization.name
    };
    let logo = if service
        .schema
        .organization
        .logo
        .as_ref()
        .is_some_and(|l| l["src"].as_str().is_some())
    {
        format!(
            "<img class=\"brand-mark\" src=\"/assets/logo\" alt=\"{}\">",
            escape(name)
        )
    } else {
        String::new()
    };
    let account = if auth.user_id.is_some() || auth.has_scope("*") {
        format!(
            "<a href=\"/account\">Account</a><a href=\"/admin\">Administration</a>{}<button class=\"quiet\">Sign out</button></form>",
            form_start("/logout", csrf)
        )
    } else {
        "<a href=\"/login\">Sign in</a>".into()
    };
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{} · {}</title><link rel=\"stylesheet\" href=\"/styles.css\"></head><body><header><a class=\"brand\" href=\"/\">{logo}<span>{}</span></a><nav><a href=\"/\">What's on</a><a href=\"/cart\">Cart</a>{account}</nav></header><main>{content}</main><footer>{}</footer></body></html>",
        escape(title_text),
        escape(name),
        escape(name),
        escape(&service.schema.organization.description)
    );
    let mut response = Html(html).into_response();
    set_cookie(&mut response, "openb2c_csrf", csrf, 86400);
    response.headers_mut().insert("content-security-policy",HeaderValue::from_static("default-src 'self'; style-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}
fn cart_tables(service: &Service) -> Result<()> {
    service.db.execute_batch("CREATE TABLE IF NOT EXISTS openb2c_browser_cart(id TEXT PRIMARY KEY,items_json TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
    Ok(())
}
fn cart_id(headers: &HeaderMap) -> Result<String> {
    cookie(headers, "openb2c_cart")
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .map(Ok)
        .unwrap_or_else(|| secret(""))
}
fn cart(service: &Service, id: &str) -> Result<Vec<Value>> {
    cart_tables(service)?;
    let text: Option<String> = service
        .db
        .query_row(
            "SELECT items_json FROM openb2c_browser_cart WHERE id=?",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(text
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}
fn save_cart(service: &Service, id: &str, items: &[Value]) -> Result<()> {
    service.db.execute("INSERT INTO openb2c_browser_cart(id,items_json) VALUES (?,?) ON CONFLICT(id) DO UPDATE SET items_json=excluded.items_json",params![id,json!(items).to_string()])?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn page(
    state: &AppState,
    service: &Service,
    method: &Method,
    path: &str,
    query: &str,
    headers: &HeaderMap,
    bytes: &[u8],
    auth: &AuthContext,
) -> Result<Response> {
    if path == "/styles.css" {
        return Ok((
            [("content-type", "text/css; charset=utf-8")],
            include_str!("../assets/app.css"),
        )
            .into_response());
    }
    if path == "/assets/logo" {
        if let Some((bytes, mime)) = state.logo.as_ref() {
            return Ok(([("content-type", mime.as_str())], bytes.clone()).into_response());
        }
        return logo(service);
    }
    let csrf = cookie(headers, "openb2c_csrf").unwrap_or(secret("")?);
    let query: BTreeMap<String, String> = serde_urlencoded::from_str(query)
        .map_err(|_| ServiceError::new("malformed", "Invalid page query"))?;
    let cart_id = cart_id(headers)?;
    let form = if method == Method::POST {
        if header(headers, "content-type")
            .split(';')
            .next()
            .unwrap_or("")
            != "application/x-www-form-urlencoded"
        {
            return Err(ServiceError::new(
                "unsupported_media_type",
                "HTML forms require application/x-www-form-urlencoded",
            ));
        }
        let form: BTreeMap<String, String> = serde_urlencoded::from_bytes(bytes)
            .map_err(|_| ServiceError::new("malformed", "Invalid form"))?;
        if form.get("_csrf") != Some(&csrf) || cookie(headers, "openb2c_csrf").is_none() {
            return Err(ServiceError::new(
                "forbidden",
                "Invalid form token; reload the page",
            ));
        }
        Some(form)
    } else if method == Method::GET {
        None
    } else {
        return Err(ServiceError::new("not_found", "Unknown page"));
    };
    let result = page_inner(
        state,
        service,
        method,
        path,
        &query,
        headers,
        form.as_ref(),
        auth,
        &csrf,
        &cart_id,
    );
    let mut response = match result {
        Ok(response) => response,
        Err(error) => {
            if error.code == "unauthorized" && method == Method::GET {
                return Ok(redirect(&format!("/login?return={}", url(path))));
            }
            let mut response = shell(
                service,
                auth,
                "Request could not be completed",
                &format!(
                    "<h1>Request could not be completed</h1><p role=\"alert\">{}</p><p><a href=\"{}\">Return</a></p>",
                    escape(&error.message),
                    escape(if method == Method::POST { path } else { "/" })
                ),
                &csrf,
            );
            *response.status_mut() =
                StatusCode::from_u16(error.status()).unwrap_or(StatusCode::BAD_REQUEST);
            response
        }
    };
    set_cookie(&mut response, "openb2c_cart", &cart_id, 30 * 86400);
    Ok(response)
}
#[allow(clippy::too_many_arguments)]
fn page_inner(
    state: &AppState,
    service: &Service,
    _method: &Method,
    path: &str,
    query: &BTreeMap<String, String>,
    headers: &HeaderMap,
    form: Option<&BTreeMap<String, String>>,
    auth: &AuthContext,
    csrf: &str,
    cart_id: &str,
) -> Result<Response> {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if path == "/login" {
        let return_to = query
            .get("return")
            .cloned()
            .or_else(|| form.and_then(|f| f.get("return").cloned()))
            .unwrap_or_else(|| "/account".into());
        if let Some(form) = form {
            if form.get("action").map(String::as_str) == Some("api-key") {
                let token = form.get("token").map(String::as_str).unwrap_or("").trim();
                service.authenticate(token)?;
                let mut response = redirect(&return_to);
                set_cookie(&mut response, "openb2c_session", token, 30 * 86400);
                return Ok(response);
            }
            if form.get("action").map(String::as_str) == Some("verify") {
                let id = form
                    .get("challengeId")
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| ServiceError::new("invalid", "Challenge is missing"))?;
                let proof = cookie(headers, "openb2c_login")
                    .ok_or_else(|| ServiceError::new("invalid", "Login cookie expired"))?;
                let result = service.verify_challenge(
                    &state.registry,
                    id,
                    form.get("code").map(String::as_str).unwrap_or(""),
                    "",
                    Some(&proof),
                )?;
                let mut response = redirect(&return_to);
                set_cookie(
                    &mut response,
                    "openb2c_session",
                    result["sessionToken"].as_str().unwrap_or(""),
                    30 * 86400,
                );
                set_cookie(&mut response, "openb2c_login", "", 0);
                return Ok(response);
            }
            let email = form
                .get("email")
                .ok_or_else(|| ServiceError::new("invalid", "Email is required"))?
                .trim()
                .to_lowercase();
            let proof = secret("")?;
            let (id, code) = service.challenge(
                &email,
                &format!("browser:{proof}"),
                crate::web::client_ip(headers),
            )?;
            service.deliver_identity_email(&email, &code, id)?;
            let content = format!(
                "<h1>Sign in</h1><p>Enter the code sent to {}.</p>{}<input type=\"hidden\" name=\"action\" value=\"verify\"><input type=\"hidden\" name=\"challengeId\" value=\"{id}\"><input type=\"hidden\" name=\"return\" value=\"{}\"><label>Verification code<input name=\"code\" inputmode=\"numeric\" pattern=\"[0-9]{{6}}\" required autocomplete=\"one-time-code\" value=\"{}\"></label><button>Sign in</button></form>",
                escape(&email),
                form_start("/login", csrf),
                escape(&return_to),
                if !production() && fake_allowed() {
                    code
                } else {
                    String::new()
                }
            );
            let mut response = shell(service, auth, "Sign in", &content, csrf);
            set_cookie(&mut response, "openb2c_login", &proof, 600);
            return Ok(response);
        }
        return Ok(shell(
            service,
            auth,
            "Sign in",
            &format!(
                "<h1>Sign in</h1>{}<input type=\"hidden\" name=\"return\" value=\"{}\"><label>Email<input type=\"email\" name=\"email\" required autocomplete=\"email\"></label><button>Send code</button></form><details><summary>Sign in with an API key</summary>{}<input type=\"hidden\" name=\"action\" value=\"api-key\"><input type=\"hidden\" name=\"return\" value=\"{}\"><label>API key<input type=\"password\" name=\"token\" required autocomplete=\"off\"></label><button>Sign in</button></form></details>",
                form_start("/login", csrf),
                escape(&return_to),
                form_start("/login", csrf),
                escape(&return_to)
            ),
            csrf,
        ));
    }
    if path == "/logout" && form.is_some() {
        if let Some(token) = cookie(headers, "openb2c_session").filter(|s| s.starts_with("sess_")) {
            service.revoke_session(&token)?;
        }
        let mut response = redirect("/");
        set_cookie(&mut response, "openb2c_session", "", 0);
        return Ok(response);
    }
    if path == "/" || path == "/catalog" {
        let mut content = format!(
            "<p class=\"eyebrow\">{}</p><h1>Performances</h1><p>{}</p>",
            escape(&service.schema.organization.name),
            escape(&service.schema.organization.description)
        );
        if let Ok(c) = service.commerce() {
            let catalog = service.catalog()?;
            content.push_str("<section class=\"cards\">");
            for item in catalog["items"].as_array().into_iter().flatten() {
                let name = service.catalog_label(&c, item)?;
                let date = item
                    .get("starts_at")
                    .map(|v| timestamp(v.as_str().unwrap_or("")))
                    .unwrap_or_default();
                content.push_str(&format!("<article><p class=\"eyebrow\">{}</p><h2><a href=\"/catalog/{}\">{}</a></h2><p>{}</p><p class=\"price\">From {}</p><a class=\"button\" href=\"/catalog/{}\">Select performance</a></article>",escape(&date),escape(&display(&item["id"])),escape(&name),escape(item.get(c.catalog.field("description")).and_then(Value::as_str).unwrap_or("")),money(item[c.catalog.field("price")].as_i64().unwrap_or(0)),escape(&display(&item["id"]))));
            }
            content.push_str("</section>");
        } else {
            for (table, ops) in &service.schema.operations {
                if ops.get("read").is_some_and(|op| op.public) && !table.starts_with("identity_") {
                    let items = service.list(table, &ListOptions::default(), auth)?;
                    content.push_str(&format!(
                        "<h2>{}</h2>{}",
                        escape(&title(table)),
                        table_html(service, table, &items["items"], auth)?
                    ));
                }
            }
        }
        return Ok(shell(service, auth, "Performances", &content, csrf));
    }
    if let ["catalog", id] = parts.as_slice() {
        let c = service.commerce()?;
        let item = service.raw(&c.catalog.entity, &crate::web::parse_id(id)?)?;
        if c.catalog.fields.contains_key("status") && item[c.catalog.field("status")] != c.available
        {
            return Err(ServiceError::new("not_found", "Performance is unavailable"));
        }
        let name = service.catalog_label(&c, &item)?;
        let mut content = format!(
            "<p><a href=\"/\">All performances</a></p><h1>Select {}</h1><p>{}</p><p class=\"price\">{}</p>{}<input type=\"hidden\" name=\"action\" value=\"add\"><input type=\"hidden\" name=\"item_id\" value=\"{}\"><label>Quantity<input type=\"number\" name=\"quantity\" value=\"1\" min=\"1\" max=\"{}\" required></label>",
            escape(&name),
            escape(&text("starts_at", &item["starts_at"], None)),
            money(item[c.catalog.field("price")].as_i64().unwrap_or(0)),
            form_start("/cart", csrf),
            escape(id),
            c.max_quantity
        );
        for field in &c.options {
            content.push_str(&widget(
                service,
                &c.line_item.entity,
                field,
                &service.schema.tables[&c.line_item.entity][field],
                &Value::Null,
                &format!("option_{field}"),
                auth,
            )?);
        }
        content.push_str("<button>Add to cart</button></form>");
        return Ok(shell(service, auth, &name, &content, csrf));
    }
    if path == "/cart" {
        let c = service.commerce()?;
        let mut items = cart(service, cart_id)?;
        if let Some(form) = form {
            match form.get("action").map(String::as_str).unwrap_or("") {
                "add" => {
                    let id = form
                        .get("item_id")
                        .ok_or_else(|| ServiceError::new("invalid", "Item is required"))?
                        .parse::<i64>()
                        .map_err(|_| ServiceError::new("invalid", "Invalid item"))?;
                    let quantity = form
                        .get("quantity")
                        .and_then(|s| s.parse::<i64>().ok())
                        .filter(|q| *q > 0 && *q <= c.max_quantity)
                        .ok_or_else(|| {
                            ServiceError::new("invalid", "Quantity is outside checkout limits")
                        })?;
                    if items.len() >= c.max_lines {
                        return Err(ServiceError::new("invalid", "Cart has too many lines"));
                    }
                    let options =
                        parse_fields(service, &c.line_item.entity, form, "option_", false)?;
                    items.push(json!({"item_id":id,"quantity":quantity,"options":options}));
                    save_cart(service, cart_id, &items)?;
                    return Ok(redirect("/cart"));
                }
                "remove" => {
                    let index = form
                        .get("index")
                        .and_then(|s| s.parse::<usize>().ok())
                        .ok_or_else(|| ServiceError::new("invalid", "Invalid cart line"))?;
                    if index < items.len() {
                        items.remove(index);
                    }
                    save_cart(service, cart_id, &items)?;
                    return Ok(redirect("/cart"));
                }
                "checkout" => {
                    require_user(auth)?;
                    let result = service.checkout(&json!({"items":items}), auth)?;
                    save_cart(service, cart_id, &[])?;
                    return Ok(redirect(&format!("/orders/{}", result["order_id"])));
                }
                _ => return Err(ServiceError::new("invalid", "Unknown cart action")),
            }
        }
        let mut content = String::from("<h1>Review cart</h1>");
        let mut total = 0i64;
        for (index, line) in items.iter().enumerate() {
            let item = service.raw(&c.catalog.entity, &line["item_id"])?;
            let amount = item[c.catalog.field("price")]
                .as_i64()
                .unwrap_or(0)
                .checked_mul(line["quantity"].as_i64().unwrap_or(0))
                .ok_or_else(|| ServiceError::new("invalid", "Cart total is out of range"))?;
            total = total
                .checked_add(amount)
                .ok_or_else(|| ServiceError::new("invalid", "Cart total is out of range"))?;
            content.push_str(&format!(
                "<article><h2>{}</h2><p>{} × {}</p>",
                escape(&service.catalog_label(&c, &item)?),
                line["quantity"],
                money(item[c.catalog.field("price")].as_i64().unwrap_or(0))
            ));
            for (name, value) in line["options"].as_object().into_iter().flatten() {
                content.push_str(&format!(
                    "<p>{}: {}</p>",
                    escape(&title(name)),
                    escape(&display(value))
                ));
            }
            content.push_str(&format!("{}<input type=\"hidden\" name=\"action\" value=\"remove\"><input type=\"hidden\" name=\"index\" value=\"{index}\"><button class=\"quiet\">Remove</button></form></article>",form_start("/cart",csrf)));
        }
        content.push_str(&format!("<p class=\"price\">Total {}</p>", money(total)));
        if !items.is_empty() {
            if auth.user_id.is_some() || auth.has_scope("*") {
                content.push_str(&format!("{}<input type=\"hidden\" name=\"action\" value=\"checkout\"><button>Reserve and continue</button></form>",form_start("/cart",csrf)));
            } else {
                content.push_str(
                    "<a class=\"button\" href=\"/login?return=%2Fcart\">Sign in to checkout</a>",
                );
            }
        } else {
            content.push_str("<p>Your cart is empty.</p><a href=\"/\">What's on</a>");
        }
        return Ok(shell(service, auth, "Cart", &content, csrf));
    }
    if let ["orders", id] = parts.as_slice() {
        require_user(auth)?;
        let c = service.commerce()?;
        let order = service.find(&c.order.entity, &crate::web::parse_id(id)?, auth)?;
        let status = order[c.order.field("status")].as_str().unwrap_or("");
        if let Some(form) = form {
            if form.get("action").map(String::as_str) == Some("payment-session") {
                service.payment_session(&order["id"], auth)?;
                return Ok(redirect(path));
            }
            if form.get("action").map(String::as_str) == Some("simulate") {
                if !fake_allowed() {
                    return Err(ServiceError::new(
                        "forbidden",
                        "Local payment simulator is disabled",
                    ));
                }
                service.authorize(&c.order.entity, "update", auth, Some(&order))?;
                let status = form
                    .get("status")
                    .map(String::as_str)
                    .unwrap_or("authorized");
                service.payment_webhook(&json!({"event_id":secret("sim_")?,"reference":order[c.order.field("paymentReference")],"status":status,"provider":std::env::var("PAYMENT_PROVIDER").unwrap_or_else(|_|"local".into())}))?;
                return Ok(redirect(path));
            }
        }
        let mut content = format!(
            "<h1>{}</h1><p>Booking #{id} · {}</p><p class=\"price\">{}</p>",
            match status {
                "paid" => "Booking confirmed",
                "payment_pending" => "Complete payment",
                "expired" => "Booking expired",
                "refund_required" => "Refund required",
                "refunded" => "Payment refunded",
                "cancelled" => "Booking cancelled",
                _ => "Payment details",
            },
            escape(status),
            money(order[c.order.field("amount")].as_i64().unwrap_or(0))
        );
        if status == "checkout_pending" {
            content.push_str(&format!("{}<input type=\"hidden\" name=\"action\" value=\"payment-session\"><button>Continue to payment</button></form>",form_start(path,csrf)));
        }
        if status == "payment_pending" {
            content.push_str(&format!("<p><a class=\"button\" href=\"/commerce/payments/local-simulator?reference={}\">Open local payment simulator</a></p>",url(order[c.order.field("paymentReference")].as_str().unwrap_or(""))));
        }
        if status == "paid" {
            content.push_str("<p>Receipt queued</p><a href=\"/account\">View my tickets</a>");
        }
        return Ok(shell(service, auth, "Booking", &content, csrf));
    }
    if path == "/commerce/payments/local-simulator" {
        require_user(auth)?;
        if !fake_allowed() {
            return Err(ServiceError::new(
                "forbidden",
                "Local payments are disabled",
            ));
        }
        let c = service.commerce()?;
        let reference = query
            .get("reference")
            .ok_or_else(|| ServiceError::new("invalid", "Payment reference is required"))?;
        let order = service
            .query(
                &format!(
                    "SELECT * FROM {} WHERE {}=?",
                    openb2c_schema::sql::quote(&c.order.entity),
                    openb2c_schema::sql::quote(c.order.field("paymentReference"))
                ),
                &[rusqlite::types::Value::Text(reference.clone())],
            )?
            .into_iter()
            .next()
            .ok_or_else(|| ServiceError::new("not_found", "Payment not found"))?;
        service.authorize(&c.order.entity, "update", auth, Some(&order))?;
        let mut content = format!(
            "<h1>Local payment simulator</h1><p>{}</p><p>{}</p>",
            money(order[c.order.field("amount")].as_i64().unwrap_or(0)),
            escape(reference)
        );
        for (status, label) in [
            ("authorized", "Authorize and capture"),
            ("captured", "Mark captured"),
            ("failed", "Fail payment"),
            ("cancelled", "Cancel payment"),
            ("refunded", "Refund payment"),
        ] {
            content.push_str(&format!("{}<input type=\"hidden\" name=\"action\" value=\"simulate\"><input type=\"hidden\" name=\"status\" value=\"{status}\"><button>{label}</button></form>",form_start(&format!("/orders/{}",order["id"]),csrf)));
        }
        return Ok(shell(
            service,
            auth,
            "Local payment simulator",
            &content,
            csrf,
        ));
    }
    if path == "/account" {
        require_user(auth)?;
        let user = auth.user_id.ok_or_else(|| {
            ServiceError::new("invalid", "Use a user-bound session to view an account")
        })?;
        if let Some(form) = form {
            let data = parse_fields(service, "user", form, "", false)?;
            service.mutate(
                "user",
                &json!(user),
                "update",
                &Value::Object(data),
                auth,
                form.get("_etag").map(String::as_str),
            )?;
            return Ok(redirect(path));
        }
        let row = service.find("user", &json!(user), auth)?;
        let mut content = format!(
            "<h1>Account</h1><h2>{}</h2>{}",
            escape(&label(&row)),
            edit_form(service, "user", &row, path, auth, csrf, false)?
        );
        if let Ok(c) = service.commerce() {
            for (heading, table) in [
                ("My tickets", &c.line_item.entity),
                ("Bookings", &c.order.entity),
            ] {
                let items = service.list(table, &ListOptions::default(), auth)?;
                content.push_str(&format!(
                    "<h2>{heading}</h2>{}",
                    table_html(service, table, &items["items"], auth)?
                ));
                if table == &c.order.entity {
                    for order in items["items"].as_array().into_iter().flatten() {
                        content.push_str(&format!(
                            "<a href=\"/orders/{}\">Booking #{}</a> ",
                            order["id"], order["id"]
                        ));
                    }
                }
            }
        }
        for profile in inference::report(&service.schema)["profileExtensions"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if profile["parentEntity"] != "user" {
                continue;
            }
            let table = profile["entity"].as_str().unwrap_or("");
            let rows = service.list(table, &ListOptions::default(), auth)?;
            let row = rows["items"]
                .as_array()
                .and_then(|r| r.first())
                .cloned()
                .unwrap_or(json!({}));
            content.push_str(&format!(
                "<h2>{}</h2>{}",
                escape(&title(table)),
                edit_form(
                    service,
                    table,
                    &row,
                    &format!("/account/profiles/{table}"),
                    auth,
                    csrf,
                    row["id"].is_null()
                )?
            ));
        }
        return Ok(shell(service, auth, "Account", &content, csrf));
    }
    if let ["account", "profiles", table] = parts.as_slice() {
        require_user(auth)?;
        let form =
            form.ok_or_else(|| ServiceError::new("not_found", "Profile form requires POST"))?;
        let user = auth
            .user_id
            .ok_or_else(|| ServiceError::new("unauthorized", "User session required"))?;
        let profile = inference::report(&service.schema)["profileExtensions"]
            .as_array()
            .and_then(|v| {
                v.iter()
                    .find(|p| p["entity"] == *table && p["parentEntity"] == "user")
            })
            .cloned()
            .ok_or_else(|| ServiceError::new("not_found", "Unknown profile extension"))?;
        let rows = service.list(table, &ListOptions::default(), auth)?;
        let existing = rows["items"].as_array().and_then(|v| v.first());
        let mut data = parse_fields(service, table, form, "", existing.is_none())?;
        data.insert(
            profile["field"].as_str().unwrap_or("user_id").into(),
            json!(user),
        );
        if let Some(row) = existing {
            service.mutate(
                table,
                &row["id"],
                "update",
                &Value::Object(data),
                auth,
                None,
            )?;
        } else {
            service.create(table, &Value::Object(data), auth)?;
        }
        return Ok(redirect("/account"));
    }
    if parts.first() == Some(&"admin") {
        require_user(auth)?;
        return admin_page(service, auth, &parts[1..], query, form, csrf);
    }
    Err(ServiceError::new("not_found", "Page not found"))
}

fn ordered_fields<'a>(service: &'a Service, table: &str) -> Result<Vec<(&'a String, &'a Column)>> {
    let mut fields = service
        .columns(table)?
        .iter()
        .filter(|(_, c)| !private(c))
        .collect::<Vec<_>>();
    fields.sort_by(|(an, a), (bn, b)| {
        a.metadata["displayPriority"]
            .as_i64()
            .unwrap_or(i64::MAX)
            .cmp(&b.metadata["displayPriority"].as_i64().unwrap_or(i64::MAX))
            .then(an.cmp(bn))
    });
    Ok(fields)
}
fn field_label(field: &str, col: &Column) -> String {
    col.metadata["label"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| title(field.strip_suffix("_id").unwrap_or(field)))
}
fn field_value(
    service: &Service,
    table: &str,
    field: &str,
    col: &Column,
    row: &Value,
    auth: &AuthContext,
) -> Result<String> {
    if let Some((target, _)) = col
        .references
        .as_deref()
        .and_then(openb2c_schema::reference)
        && let Ok(target) = service.find(target, &row[field], auth)
    {
        return Ok(label(&target));
    }
    let _ = table;
    Ok(text(field, &row[field], Some(col)))
}
fn table_html(service: &Service, table: &str, items: &Value, auth: &AuthContext) -> Result<String> {
    let fields = ordered_fields(service, table)?
        .into_iter()
        .take(7)
        .collect::<Vec<_>>();
    let mut html = String::from("<div class=\"table-scroll\"><table><thead><tr>");
    for (field, col) in &fields {
        html.push_str(&format!("<th>{}</th>", escape(&field_label(field, col))));
    }
    html.push_str("<th>Details</th></tr></thead><tbody>");
    for row in items.as_array().into_iter().flatten() {
        html.push_str("<tr>");
        for (field, col) in &fields {
            html.push_str(&format!(
                "<td>{}</td>",
                escape(&field_value(service, table, field, col, row, auth)?)
            ));
        }
        html.push_str(&format!(
            "<td><a href=\"/admin/{}/{}\">Open</a></td></tr>",
            escape(table),
            escape(&display(&row[service.primary_key(table)?]))
        ));
    }
    html.push_str("</tbody></table></div>");
    Ok(html)
}
fn widget(
    service: &Service,
    table: &str,
    field: &str,
    col: &Column,
    value: &Value,
    name: &str,
    auth: &AuthContext,
) -> Result<String> {
    let default = col
        .default
        .as_deref()
        .filter(|s| *s != "CURRENT_TIMESTAMP")
        .map(|s| s.trim_matches('\'').to_string())
        .unwrap_or_default();
    let value = if value.is_null() {
        default
    } else {
        display(value)
    };
    let required = if col.required && col.default.is_none() {
        " required"
    } else {
        ""
    };
    let label = escape(&field_label(field, col));
    let name = escape(name);
    let value = escape(&value);
    let mut html = format!("<label>{label}");
    if !col.constraints.values.is_empty() {
        html.push_str(&format!("<select name=\"{name}\"{required}>"));
        if !col.required {
            html.push_str("<option value=\"\">None</option>");
        }
        for option in &col.constraints.values {
            html.push_str(&format!(
                "<option value=\"{}\"{}>{}</option>",
                escape(option),
                if escape(option) == value {
                    " selected"
                } else {
                    ""
                },
                escape(&title(option))
            ));
        }
        html.push_str("</select>");
    } else if let Some((target, _)) = col
        .references
        .as_deref()
        .and_then(openb2c_schema::reference)
    {
        if let Ok(rows) = service.list(
            target,
            &ListOptions {
                limit: 1000,
                ..Default::default()
            },
            auth,
        ) {
            html.push_str(&format!(
                "<select name=\"{name}\"{required}><option value=\"\">Select {label}</option>"
            ));
            for row in rows["items"].as_array().into_iter().flatten() {
                let id = display(&row[service.primary_key(target)?]);
                html.push_str(&format!(
                    "<option value=\"{}\"{}>{}</option>",
                    escape(&id),
                    if escape(&id) == value {
                        " selected"
                    } else {
                        ""
                    },
                    escape(&crate::commerce::label(row))
                ));
            }
            html.push_str("</select>");
        } else {
            html.push_str(&format!(
                "<input name=\"{name}\" type=\"number\" value=\"{value}\"{required}>"
            ));
        }
    } else {
        let format = col.metadata["format"].as_str().unwrap_or("");
        let input_type = if ["email", "url", "date", "time"].contains(&format) {
            format
        } else if matches!(col.kind, SqlType::Integer | SqlType::Real) {
            "number"
        } else if format == "date-time" || field.ends_with("_at") {
            "datetime-local"
        } else {
            "text"
        };
        let value = if input_type == "datetime-local" {
            value.trim_end_matches('Z').to_string()
        } else {
            value
        };
        if field == "description" || field == "notes" || field == "bio" {
            html.push_str(&format!(
                "<textarea name=\"{name}\"{required}>{value}</textarea>"
            ));
        } else {
            html.push_str(&format!(
                "<input name=\"{name}\" type=\"{input_type}\" value=\"{value}\"{required}"
            ));
            if input_type == "number" {
                html.push_str(if col.kind == SqlType::Integer {
                    " step=\"1\""
                } else {
                    " step=\"any\""
                });
            }
            for (bounds, low, high) in [
                (&col.constraints.range, "min", "max"),
                (&col.constraints.length, "minlength", "maxlength"),
            ] {
                if let Some(bounds) = bounds {
                    if let Some(v) = bounds.min {
                        html.push_str(&format!(" {low}=\"{v}\""));
                    }
                    if let Some(v) = bounds.max {
                        html.push_str(&format!(" {high}=\"{v}\""));
                    }
                }
            }
            if let Some(pattern) = &col.constraints.pattern {
                html.push_str(&format!(" pattern=\"{}\"", escape(pattern)));
            }
            if let Some(placeholder) = col.metadata["placeholder"].as_str() {
                html.push_str(&format!(" placeholder=\"{}\"", escape(placeholder)));
            }
            html.push('>');
        }
    }
    if let Some(help) = col.metadata["helpText"].as_str() {
        html.push_str(&format!("<small>{}</small>", escape(help)));
    }
    html.push_str("</label>");
    let _ = table;
    Ok(html)
}
fn edit_form(
    service: &Service,
    table: &str,
    row: &Value,
    action: &str,
    auth: &AuthContext,
    csrf: &str,
    create: bool,
) -> Result<String> {
    let mut html = form_start(action, csrf);
    if !create {
        html.push_str(&format!(
            "<input type=\"hidden\" name=\"_etag\" value=\"{}\">",
            escape(&crate::service::etag(table, row))
        ));
    }
    for (field, col) in ordered_fields(service, table)? {
        if col.pk
            || [
                "created_at",
                "updated_at",
                "key_hash",
                "key_prefix",
                "last_used_at",
            ]
            .contains(&field.as_str())
        {
            continue;
        }
        if table == "api_key" && field == "scopes" && !auth.has_scope("*") {
            continue;
        }
        html.push_str(&widget(
            service,
            table,
            field,
            col,
            &row[field],
            field,
            auth,
        )?);
    }
    html.push_str("<button>Save</button></form>");
    Ok(html)
}
fn parse_fields(
    service: &Service,
    table: &str,
    form: &BTreeMap<String, String>,
    prefix: &str,
    create: bool,
) -> Result<Map<String, Value>> {
    let mut data = Map::new();
    for (field, col) in service.columns(table)? {
        if col.pk
            || private(col)
            || [
                "created_at",
                "updated_at",
                "key_hash",
                "key_prefix",
                "last_used_at",
            ]
            .contains(&field.as_str())
        {
            continue;
        }
        let Some(value) = form.get(&format!("{prefix}{field}")) else {
            continue;
        };
        if value.is_empty() {
            if !create && !col.required {
                data.insert(field.clone(), Value::Null);
            }
            continue;
        }
        let value = match col.kind {
            SqlType::Integer => json!(value.parse::<i64>().map_err(|_| ServiceError::new(
                "invalid",
                format!("{field} must be an integer")
            ))?),
            SqlType::Real => {
                let n = value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| {
                        ServiceError::new("invalid", format!("{field} must be a number"))
                    })?;
                json!(n)
            }
            SqlType::Blob => {
                return Err(ServiceError::new(
                    "invalid",
                    "Binary fields require the JSON API",
                ));
            }
            SqlType::Text => {
                let value = if (field.ends_with("_at") || col.metadata["format"] == "date-time")
                    && chrono::DateTime::parse_from_rfc3339(value).is_err()
                {
                    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
                        .or_else(|_| {
                            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                        })
                        .map(|t| t.and_utc().to_rfc3339())
                        .unwrap_or_else(|_| value.clone())
                } else {
                    value.clone()
                };
                json!(value)
            }
        };
        data.insert(field.clone(), value);
    }
    Ok(data)
}
fn admin_page(
    service: &Service,
    auth: &AuthContext,
    parts: &[&str],
    query: &BTreeMap<String, String>,
    form: Option<&BTreeMap<String, String>>,
    csrf: &str,
) -> Result<Response> {
    let mut nav = String::from("<aside><h2>Workspaces</h2><a href=\"/admin\">Overview</a>");
    for table in service.schema.tables.keys() {
        if !table.starts_with("identity_") && service.authorize(table, "read", auth, None).is_ok() {
            nav.push_str(&format!(
                "<a href=\"/admin/{}\">{}</a>",
                escape(table),
                escape(&title(table))
            ));
        }
    }
    nav.push_str("</aside><section class=\"workspace\">");
    if parts.is_empty() {
        nav.push_str("<h1>Administration</h1><p>Select a workspace to manage its records and workflows.</p></section>");
        return Ok(shell(
            service,
            auth,
            "Administration",
            &format!("<div class=\"admin\">{nav}</div>"),
            csrf,
        ));
    }
    let table = parts[0];
    service.columns(table)?;
    if table.starts_with("identity_") {
        return Err(ServiceError::new(
            "forbidden",
            "Identity storage is private",
        ));
    }
    let base = format!("/admin/{table}");
    if parts.len() == 1 {
        let mut options = ListOptions::default();
        for (field, value) in query {
            if service.columns(table)?.contains_key(field) {
                let col = &service.columns(table)?[field];
                let value =
                    if col.kind == SqlType::Integer {
                        json!(value.parse::<i64>().map_err(|_| ServiceError::new(
                            "invalid",
                            "Invalid relationship filter"
                        ))?)
                    } else {
                        json!(value)
                    };
                options.filter.insert(field.clone(), value);
            }
        }
        let rows = service.list(table, &options, auth)?;
        nav.push_str(&format!("<h1>{}</h1><div class=\"actions\"><a class=\"button\" href=\"{base}/new\">New {}</a><a href=\"{base}\">List</a><a href=\"{base}?view=board\">Board</a><a href=\"{base}?view=calendar\">Calendar</a></div>",escape(&title(table)),escape(&title(table))));
        match query.get("view").map(String::as_str) {
            Some("board") => {
                if let Some((field, col)) = service
                    .columns(table)?
                    .iter()
                    .find(|(_, c)| !c.constraints.values.is_empty() && c.kind == SqlType::Text)
                {
                    nav.push_str("<div class=\"board\">");
                    for status in &col.constraints.values {
                        nav.push_str(&format!("<section><h2>{}</h2>", escape(&title(status))));
                        for row in rows["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|r| r[field] == *status)
                        {
                            nav.push_str(&format!(
                                "<article><a href=\"{base}/{}\">{}</a>{}</article>",
                                row["id"],
                                escape(&crate::commerce::label(row)),
                                operation_links(service, table, row, auth)?
                            ));
                        }
                        nav.push_str("</section>");
                    }
                    nav.push_str("</div>");
                } else {
                    nav.push_str(&table_html(service, table, &rows["items"], auth)?);
                }
            }
            Some("calendar") => {
                let field = ["starts_at", "start_at", "scheduled_at", "date"]
                    .into_iter()
                    .find(|f| service.columns(table).is_ok_and(|c| c.contains_key(*f)));
                if let Some(field) = field {
                    let mut items = rows["items"].as_array().cloned().unwrap_or_default();
                    items.sort_by_key(|r| display(&r[field]));
                    nav.push_str("<ol class=\"calendar\">");
                    for row in items {
                        nav.push_str(&format!(
                            "<li><time>{}</time> <a href=\"{base}/{}\">{}</a></li>",
                            escape(&text(field, &row[field], None)),
                            row["id"],
                            escape(&label(&row))
                        ));
                    }
                    nav.push_str("</ol>");
                } else {
                    nav.push_str("<p>This entity has no temporal fields.</p>");
                }
            }
            _ => nav.push_str(&table_html(service, table, &rows["items"], auth)?),
        }
    } else if parts[1] == "new" {
        service.authorize(table, "create", auth, None)?;
        if let Some(form) = form {
            let input = parse_fields(service, table, form, "", true)?;
            let row = service.create(table, &Value::Object(input), auth)?;
            if let Some(key) = row["key"].as_str() {
                nav.push_str(&format!("<h1>API key created</h1><p>Copy this key now. It is shown once.</p><code>{}</code>",escape(key)));
            } else {
                return Ok(redirect(&format!(
                    "{base}/{}",
                    display(&row[service.primary_key(table)?])
                )));
            }
        } else {
            nav.push_str(&format!(
                "<h1>New {}</h1>{}",
                escape(&title(table)),
                edit_form(
                    service,
                    table,
                    &json!({}),
                    &format!("{base}/new"),
                    auth,
                    csrf,
                    true
                )?
            ));
        }
    } else {
        let id = if service.columns(table)?[service.primary_key(table)?].kind == SqlType::Integer {
            crate::web::parse_id(parts[1])?
        } else {
            json!(parts[1])
        };
        let row = service.find(table, &id, auth)?;
        if parts.len() >= 4 && parts[2] == "operations" {
            let name = parts[3].replace('-', "_");
            let op = service
                .schema
                .operations
                .get(table)
                .and_then(|o| o.get(&name))
                .filter(|_| !["read", "create", "update", "delete"].contains(&name.as_str()))
                .ok_or_else(|| ServiceError::new("not_found", "Unknown workflow operation"))?;
            service.authorize(table, &name, auth, Some(&row))?;
            if let Some(form) = form {
                let input = parse_fields(service, table, form, "", false)?;
                service.mutate(
                    table,
                    &id,
                    &name,
                    &Value::Object(input),
                    auth,
                    form.get("_etag").map(String::as_str),
                )?;
                return Ok(redirect(&format!("{base}/{}", parts[1])));
            }
            let confirmation = &op.presentation["confirmation"];
            nav.push_str(&format!(
                "<h1>{}</h1><p>{}</p>{}<input type=\"hidden\" name=\"_etag\" value=\"{}\">",
                escape(
                    confirmation["title"]
                        .as_str()
                        .or(op.presentation["label"].as_str())
                        .unwrap_or(&name)
                ),
                escape(
                    confirmation["message"]
                        .as_str()
                        .or(op.presentation["description"].as_str())
                        .unwrap_or("Apply this operation to the record?")
                ),
                form_start(&format!("{base}/{}/operations/{name}", parts[1]), csrf),
                escape(&crate::service::etag(table, &row))
            ));
            for (field, assignment) in &op.set {
                if assignment.input == Some(true) {
                    nav.push_str(&widget(
                        service,
                        table,
                        field,
                        &service.columns(table)?[field],
                        &row[field],
                        field,
                        auth,
                    )?);
                }
            }
            nav.push_str(&format!(
                "<button class=\"{}\">{}</button></form>",
                escape(confirmation["severity"].as_str().unwrap_or("info")),
                escape(confirmation["confirmLabel"].as_str().unwrap_or("Confirm"))
            ));
        } else if parts.get(2) == Some(&"delete") {
            service.authorize(table, "delete", auth, Some(&row))?;
            if let Some(form) = form {
                service.mutate(
                    table,
                    &id,
                    "delete",
                    &json!({}),
                    auth,
                    form.get("_etag").map(String::as_str),
                )?;
                return Ok(redirect(&base));
            }
            nav.push_str(&format!("<h1>Delete {}</h1><p>This will delete {}.</p>{}<input type=\"hidden\" name=\"_etag\" value=\"{}\"><button class=\"danger\">Delete record</button></form>",escape(&title(table)),escape(&label(&row)),form_start(&format!("{base}/{}/delete",parts[1]),csrf),escape(&crate::service::etag(table,&row))));
        } else {
            if let Some(form) = form {
                let input = parse_fields(service, table, form, "", false)?;
                service.mutate(
                    table,
                    &id,
                    "update",
                    &Value::Object(input),
                    auth,
                    form.get("_etag").map(String::as_str),
                )?;
                return Ok(redirect(&format!("{base}/{}", parts[1])));
            }
            nav.push_str(&format!(
                "<p><a href=\"{base}\">All {}</a></p><h1>{}</h1>{}",
                escape(&title(table)),
                escape(&label(&row)),
                operation_links(service, table, &row, auth)?
            ));
            if service.authorize(table, "update", auth, Some(&row)).is_ok() {
                nav.push_str(&edit_form(
                    service,
                    table,
                    &row,
                    &format!("{base}/{}", parts[1]),
                    auth,
                    csrf,
                    false,
                )?);
            } else {
                nav.push_str(&table_html(service, table, &json!([row.clone()]), auth)?);
            }
            for (child, cols) in &service.schema.tables {
                for (field, col) in cols {
                    if col
                        .references
                        .as_deref()
                        .and_then(openb2c_schema::reference)
                        .is_none_or(|(t, _)| t != table)
                    {
                        continue;
                    }
                    let options = ListOptions {
                        filter: BTreeMap::from([(field.clone(), id.clone())]),
                        ..Default::default()
                    };
                    if let Ok(children) = service.list(child, &options, auth) {
                        nav.push_str(&format!(
                            "<h2>{} · {}</h2>{}",
                            escape(&title(child)),
                            escape(&field_label(field, col)),
                            table_html(service, child, &children["items"], auth)?
                        ));
                    }
                }
            }
            if service.authorize(table, "delete", auth, Some(&row)).is_ok() {
                nav.push_str(&format!(
                    "<p><a href=\"{base}/{}/delete\">Delete record…</a></p>",
                    parts[1]
                ));
            }
        }
    }
    nav.push_str("</section>");
    Ok(shell(
        service,
        auth,
        &title(table),
        &format!("<div class=\"admin\">{nav}</div>"),
        csrf,
    ))
}
fn operation_links(
    service: &Service,
    table: &str,
    row: &Value,
    auth: &AuthContext,
) -> Result<String> {
    let mut html = String::from("<div class=\"actions\">");
    let mut ops = service
        .schema
        .operations
        .get(table)
        .into_iter()
        .flatten()
        .filter(|(name, _)| !["read", "create", "update", "delete"].contains(&name.as_str()))
        .collect::<Vec<_>>();
    ops.sort_by_key(|(_, op)| {
        op.presentation["displayPriority"]
            .as_i64()
            .unwrap_or(i64::MAX)
    });
    for (name, op) in ops {
        if service.authorize(table, name, auth, Some(row)).is_err() {
            continue;
        }
        if let Some(guard) = &op.guard
            && !crate::service::truthy(&service.evaluate(table, row, guard)?)
        {
            continue;
        }
        html.push_str(&format!(
            "<a class=\"button secondary\" href=\"/admin/{}/{}/operations/{}\" title=\"{}\">{}</a>",
            escape(table),
            escape(&display(&row[service.primary_key(table)?])),
            escape(name),
            escape(op.presentation["description"].as_str().unwrap_or("")),
            escape(op.presentation["label"].as_str().unwrap_or(name))
        ));
    }
    html.push_str("</div>");
    Ok(html)
}
fn logo(service: &Service) -> Result<Response> {
    let source = service
        .schema
        .organization
        .logo
        .as_ref()
        .and_then(|l| l["src"].as_str())
        .ok_or_else(|| ServiceError::new("not_found", "Logo is unavailable"))?;
    let bytes =
        std::fs::read(source).map_err(|_| ServiceError::new("not_found", "Logo is unavailable"))?;
    let mime = if source.ends_with(".svg") {
        "image/svg+xml"
    } else if source.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };
    Ok(([("content-type", mime)], bytes).into_response())
}
fn url(value: &str) -> String {
    serde_urlencoded::to_string([("", value)])
        .unwrap_or_default()
        .trim_start_matches('=')
        .into()
}
