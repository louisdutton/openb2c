use crate::{
    AuthContext, Service,
    service::{ListOptions, Result, ServiceError},
};
use openb2c_schema::{Column, Schema, SqlType};
use serde_json::{Map, Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
};

fn input_field(field: &str, column: &Column) -> Value {
    let kind = match column.kind {
        SqlType::Integer => "integer",
        SqlType::Real => "number",
        SqlType::Text => "string",
        SqlType::Blob => "array",
    };
    let mut value = json!({"type":kind,"title":column.metadata["label"].as_str().unwrap_or(field),"description":column.metadata["helpText"].as_str().unwrap_or("")});
    if column.kind == SqlType::Blob {
        value["items"] = json!({"type":"integer","minimum":0,"maximum":255});
    }
    if !column.constraints.values.is_empty() {
        value["enum"] = json!(
            column
                .constraints
                .values
                .iter()
                .map(|s| if kind == "integer" || kind == "number" {
                    serde_json::from_str(s).unwrap_or(json!(s))
                } else {
                    json!(s)
                })
                .collect::<Vec<_>>()
        );
    }
    if let Some(pattern) = &column.constraints.pattern {
        value["pattern"] = json!(pattern);
    }
    for (bounds, min, max) in [
        (&column.constraints.length, "minLength", "maxLength"),
        (&column.constraints.range, "minimum", "maximum"),
    ] {
        if let Some(bounds) = bounds {
            if let Some(v) = bounds.min {
                value[min] = json!(v);
            }
            if let Some(v) = bounds.max {
                value[max] = json!(v);
            }
        }
    }
    value
}
pub fn tools(schema: &Schema) -> Vec<Value> {
    let mut result = Vec::new();
    for (entity, columns) in &schema.tables {
        if entity.starts_with("identity_") {
            continue;
        }
        for action in ["list", "get", "create", "update", "delete"]
            .into_iter()
            .chain(
                schema
                    .operations
                    .get(entity)
                    .into_iter()
                    .flatten()
                    .filter(|(n, _)| !["read", "create", "update", "delete"].contains(&n.as_str()))
                    .map(|(n, _)| n.as_str()),
            )
        {
            let name = if action == "list" {
                format!("list_{entity}s")
            } else {
                format!("{action}_{entity}")
            };
            let operation = schema.operations.get(entity).and_then(|ops| {
                ops.get(if action == "get" || action == "list" {
                    "read"
                } else {
                    action
                })
            });
            let mut properties = Map::new();
            let mut required = Vec::new();
            if !["list", "create"].contains(&action) {
                properties.insert(
                    "id".into(),
                    json!({"type":"integer","description":"Record identifier"}),
                );
                required.push("id".to_string());
            }
            if action == "list" {
                properties.extend(Map::from_iter([
                    (
                        "limit".into(),
                        json!({"type":"integer","minimum":1,"maximum":1000}),
                    ),
                    ("offset".into(), json!({"type":"integer","minimum":0})),
                    (
                        "sort".into(),
                        json!({"type":"string","enum":columns.keys().collect::<Vec<_>>()}),
                    ),
                    (
                        "order".into(),
                        json!({"type":"string","enum":["asc","desc"]}),
                    ),
                    ("filter".into(), json!({"type":"object"})),
                ]));
            }
            for (field, col) in columns {
                if col.pk
                    || col.metadata["privacy"] == "secret"
                    || col.metadata["redact"] == true
                    || ["created_at", "updated_at", "key_hash", "key_prefix"]
                        .contains(&field.as_str())
                {
                    continue;
                }
                let is_input = ["create", "update"].contains(&action)
                    || operation
                        .is_some_and(|op| op.set.get(field).is_some_and(|a| a.input == Some(true)));
                if is_input {
                    properties.insert(field.clone(), input_field(field, col));
                    if action == "create"
                        && col.required
                        && col.default.is_none()
                        && !operation.is_some_and(|op| {
                            op.relationships.iter().any(|r| r.field.field == *field)
                        })
                    {
                        required.push(field.clone());
                    }
                }
            }
            if action == "update"
                || action == "delete"
                || !["list", "get", "create"].contains(&action)
            {
                properties.insert(
                    "if_match".into(),
                    json!({"type":"string","description":"Optional current ETag"}),
                );
            }
            let presentation = operation
                .map(|o| o.presentation.clone())
                .unwrap_or(Value::Null);
            let description = presentation["description"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "{} {} records",
                        crate::ui::title(action),
                        crate::ui::title(entity)
                    )
                });
            result.push(json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":(["list","get"].contains(&action)),"destructiveHint":action=="delete"||action.contains("cancel")||action.contains("archive"),"idempotentHint":action!="create","openWorldHint":operation.is_some_and(|o|!o.effects.is_empty())},"_meta":{"openb2c/confirmation":presentation["confirmation"],"openb2c/presentation":presentation,"openb2c/audit":operation.map(|o|o.audit.clone()),"openb2c/policy":operation.map(|o|o.policy.clone()),"openb2c/entity":entity,"openb2c/action":if ["list","get"].contains(&action){"read"}else{action}}}));
        }
    }
    if schema
        .metadata
        .get("commerce")
        .is_some_and(|c| c["enabled"] == true)
    {
        for name in [
            "list_commerce_catalog",
            "checkout_cart",
            "create_order_payment_session",
            "expire_commerce_orders",
            "reserve_booking",
            "create_booking_payment_session",
            "expire_checkout_bookings",
        ] {
            let input = match name {
                "checkout_cart" => {
                    json!({"type":"object","properties":{"items":{"type":"array","minItems":1,"items":{"type":"object","properties":{"item_id":{"type":"integer"},"quantity":{"type":"integer","minimum":1},"options":{"type":"object"}},"required":["item_id"],"additionalProperties":false}},"user_id":{"type":"integer"},"client":{"type":"string"}},"required":["items"],"additionalProperties":false})
                }
                "reserve_booking" => {
                    json!({"type":"object","properties":{"performance_id":{"type":"integer"},"quantity":{"type":"integer","minimum":1},"ticket_type":{"type":"string"},"tickets":{"type":"array"},"user_id":{"type":"integer"},"client":{"type":"string"}},"required":["performance_id"],"additionalProperties":false})
                }
                "create_order_payment_session" | "create_booking_payment_session" => {
                    json!({"type":"object","properties":{"order_id":{"type":"integer"},"booking_id":{"type":"integer"}},"additionalProperties":false})
                }
                _ => json!({"type":"object","properties":{},"additionalProperties":false}),
            };
            result.push(json!({"name":name,"description":crate::ui::title(name),"inputSchema":input,"annotations":{"readOnlyHint":name=="list_commerce_catalog","openWorldHint":true}}));
        }
    }
    result
}
pub fn call(service: &Service, name: &str, args: &Value, auth: &AuthContext) -> Result<Value> {
    let _source = crate::service::SourceScope::enter("mcp");
    let definition = tools(&service.schema)
        .into_iter()
        .find(|tool| tool["name"] == name)
        .ok_or_else(|| ServiceError::new("not_found", "Unknown tool"))?;
    let arguments = args
        .as_object()
        .ok_or_else(|| ServiceError::new("invalid", "Tool arguments must be an object"))?;
    let spec = &definition["inputSchema"];
    if arguments
        .keys()
        .any(|key| spec["properties"].get(key).is_none())
        || spec["required"].as_array().is_some_and(|required| {
            required
                .iter()
                .any(|key| key.as_str().is_some_and(|key| !arguments.contains_key(key)))
        })
    {
        return Err(ServiceError::new(
            "invalid",
            "Missing or unknown tool arguments",
        ));
    }
    for (name, value) in arguments {
        let field = &spec["properties"][name];
        let valid = match field["type"].as_str() {
            Some("integer") => value.is_i64(),
            Some("number") => value.is_number(),
            Some("string") => value.is_string(),
            Some("array") => value.is_array(),
            Some("object") => value.is_object(),
            _ => true,
        };
        if !valid {
            return Err(ServiceError::new(
                "invalid",
                format!("{name} has the wrong type"),
            ));
        }
        if value.as_f64().is_some_and(|v| {
            field["minimum"].as_f64().is_some_and(|min| v < min)
                || field["maximum"].as_f64().is_some_and(|max| v > max)
        }) {
            return Err(ServiceError::new(
                "invalid",
                format!("{name} is outside its allowed bounds"),
            ));
        }
    }
    let value = match name {
        "list_commerce_catalog" => service.catalog()?,
        "checkout_cart" => service.checkout(args, auth)?,
        "reserve_booking" => service.reserve_booking(args, auth)?,
        "create_order_payment_session" | "create_booking_payment_session" => service
            .payment_session(
                args.get("order_id")
                    .or_else(|| args.get("booking_id"))
                    .ok_or_else(|| ServiceError::new("invalid", "Order ID is required"))?,
                auth,
            )?,
        "expire_commerce_orders" | "expire_checkout_bookings" => {
            if !auth.has_scope("commerce.expire") && !auth.has_scope("booking.expire") {
                return Err(ServiceError::new("forbidden", "Expiry permission required"));
            }
            service.expire_orders()?
        }
        _ => {
            let target = service
                .schema
                .tables
                .keys()
                .filter(|t| !t.starts_with("identity_"))
                .find_map(|table| {
                    for action in ["list", "get", "create", "update", "delete"]
                        .into_iter()
                        .chain(
                            service
                                .schema
                                .operations
                                .get(table)
                                .into_iter()
                                .flatten()
                                .filter(|(a, _)| {
                                    !["read", "create", "update", "delete"].contains(&a.as_str())
                                })
                                .map(|(a, _)| a.as_str()),
                        )
                    {
                        if name
                            == if action == "list" {
                                format!("list_{table}s")
                            } else {
                                format!("{action}_{table}")
                            }
                        {
                            return Some((table.as_str(), action));
                        }
                    }
                    None
                })
                .ok_or_else(|| ServiceError::new("not_found", "Unknown tool"))?;
            let (table, action) = target;
            match action {
                "list" => {
                    let options = ListOptions {
                        limit: args["limit"].as_u64().unwrap_or(50).min(1000) as u32,
                        offset: args["offset"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
                        sort: args["sort"].as_str().map(str::to_string),
                        descending: args["order"] == "desc",
                        filter: args["filter"]
                            .as_object()
                            .map(|m| m.clone().into_iter().collect())
                            .unwrap_or_default(),
                    };
                    service.list(table, &options, auth)?
                }
                "get" => service.find(table, &args["id"], auth)?,
                "create" => service.create(table, args, auth)?,
                _ => {
                    let mut input = args.clone();
                    if let Some(object) = input.as_object_mut() {
                        object.remove("id");
                        object.remove("if_match");
                    }
                    service.mutate(
                        table,
                        &args["id"],
                        action,
                        &input,
                        auth,
                        args["if_match"].as_str(),
                    )?
                }
            }
        }
    };
    service.dispatch_effects(false)?;
    Ok(value)
}
pub fn respond(service: &Service, request: &Value, auth: &AuthContext) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":"2025-03-26","serverInfo":{"name":crate::ui::slug(&service.schema.organization.name),"version":env!("CARGO_PKG_VERSION")},"capabilities":{"tools":{}}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":tools(&service.schema).into_iter().filter(|tool| {
            if let (Some(entity),Some(action))=(tool["_meta"]["openb2c/entity"].as_str(),tool["_meta"]["openb2c/action"].as_str()) {
                let op=service.schema.operations.get(entity).and_then(|ops|ops.get(action));
                op.is_some_and(|o|o.public)|| (auth.has_scope(op.and_then(|o|o.scope.as_deref()).filter(|s|!s.is_empty()).unwrap_or(&format!("{entity}.{action}"))) && (auth.user_id.is_some()||op.is_none_or(|o|o.relationships.is_empty())||auth.has_scope("*")))
            } else { tool["name"]=="list_commerce_catalog" || auth.user_id.is_some() || auth.has_scope("*") }
        }).collect::<Vec<_>>()}),
        "tools/call" => {
            let params = &request["params"];
            match call(
                service,
                params["name"].as_str().unwrap_or(""),
                &params.get("arguments").cloned().unwrap_or(json!({})),
                auth,
            ) {
                Ok(value) => {
                    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value})
                }
                Err(error) => {
                    json!({"isError":true,"content":[{"type":"text","text":json!({"ok":false,"code":error.code,"error":error.message}).to_string()}]})
                }
            }
        }
        _ => {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}});
        }
    };
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
pub fn stdio(schema_json: &str, root: &Path) -> anyhow::Result<()> {
    let schema: Schema = serde_json::from_str(schema_json)?;
    let service = crate::open_from_environment(schema, root)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                if request.get("id").is_none() {
                    continue;
                }
                respond(&service, &request, &AuthContext::system())
            }
            Err(_) => {
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})
            }
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}
