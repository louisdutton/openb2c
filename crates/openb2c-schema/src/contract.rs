use crate::*;
use serde_json::{Map, Value, json};
pub fn pascal(name: &str) -> String {
    let result: String = name
        .split('_')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut c = s.chars();
            c.next()
                .map(|c| c.to_uppercase().collect::<String>() + &s[1..])
                .unwrap_or_default()
        })
        .collect();
    if result.is_empty() || result.starts_with(|c: char| c.is_ascii_digit()) {
        format!("Entity{result}")
    } else {
        result
    }
}
pub fn field_schema(field: &str, c: &Column, nullable: bool) -> Value {
    let kind = match c.kind {
        SqlType::Integer => "integer",
        SqlType::Real => "number",
        SqlType::Text => "string",
        SqlType::Blob => "array",
    };
    let mut value = json!({"type":kind,"title":c.metadata["label"].as_str().unwrap_or(field),"x-openb2c-field":c.metadata,"x-openb2c-validation":c.constraints});
    if nullable && !c.required && !c.pk {
        value["type"] = json!([kind, "null"]);
    }
    if let Some(help) = c.metadata["helpText"].as_str() {
        value["description"] = json!(help);
    }
    if let Some(format) = c.metadata["format"].as_str() {
        value["format"] = json!(format);
    }
    if c.kind == SqlType::Blob {
        value["items"] = json!({"type":"integer","minimum":0,"maximum":255});
    }
    if !c.constraints.values.is_empty() {
        value["enum"] = json!(
            c.constraints
                .values
                .iter()
                .map(|s| if matches!(c.kind, SqlType::Integer | SqlType::Real) {
                    serde_json::from_str(s).unwrap_or(json!(s))
                } else {
                    json!(s)
                })
                .collect::<Vec<_>>()
        );
    }
    if let Some(pattern) = &c.constraints.pattern {
        value["pattern"] = json!(pattern);
    }
    for (bounds, min, max) in [
        (&c.constraints.length, "minLength", "maxLength"),
        (&c.constraints.range, "minimum", "maximum"),
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
    if let Some((table, key)) = c.references.as_deref().and_then(reference) {
        value["x-openb2c-relationship"] = json!({"targetEntity":table,"targetField":key,"label":c.relationship.as_ref().and_then(|r|r["label"].as_str()),"description":c.relationship.as_ref().and_then(|r|r["description"].as_str()),"cardinality":c.relationship.as_ref().and_then(|r|r["cardinality"].as_str()).unwrap_or("one")});
    }
    value
}
fn hidden(field: &str, c: &Column) -> bool {
    field.ends_with("_hash")
        || field == "signature"
        || c.metadata["redact"] == true
        || c.metadata["privacy"] == "secret"
}
pub fn input_schema(schema: &Schema, table: &str, partial: bool) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (field, c) in &schema.tables[table] {
        if c.pk && c.auto
            || ["created_at", "updated_at", "key_hash", "key_prefix"].contains(&field.as_str())
        {
            continue;
        }
        props.insert(field.clone(), field_schema(field, c, true));
        let rel = schema
            .operations
            .get(table)
            .and_then(|ops| ops.get("create"))
            .is_some_and(|op| op.relationships.iter().any(|r| r.field.field == *field));
        if !partial && c.required && c.default.is_none() && !rel {
            required.push(field.clone());
        }
    }
    json!({"type":"object","properties":props,"required":required,"additionalProperties":false})
}
fn op_metadata(schema: &Schema, table: &str, name: &str) -> Value {
    let operation = schema
        .operations
        .get(table)
        .and_then(|ops| ops.get(name))
        .cloned()
        .unwrap_or_default();
    let mut metadata = json!(operation);
    let audiences = operation.policy["audiences"]
        .as_array()
        .filter(|a| !a.is_empty())
        .cloned()
        .unwrap_or_else(|| {
            if operation.public {
                vec![json!("anonymous")]
            } else if !operation.relationships.is_empty() {
                vec![json!("customer")]
            } else {
                vec![json!("staff"), json!("service"), json!("system")]
            }
        });
    metadata["scope"] = json!(operation.scope.unwrap_or_else(|| format!("{table}.{name}")));
    metadata["policy"]["audiences"] = json!(audiences);
    metadata
}
pub fn openapi(schema: &Schema) -> Value {
    let name = if schema.organization.name.is_empty() {
        "OpenB2C"
    } else {
        &schema.organization.name
    };
    let graph = inference::report(schema);
    let mut doc = json!({"openapi":"3.1.0","info":{"title":name,"description":schema.organization.description,"version":"0.1.0"},"paths":{},"components":{"schemas":{},"securitySchemes":{"bearerAuth":{"type":"http","scheme":"bearer"},"certificateAuth":{"type":"apiKey","in":"header","name":"X-Certificate"}}},"x-openb2c":graph,"x-openb2c-app":{"name":name,"version":"0.1.0","organization":schema.organization},"x-openb2c-auth":schema.metadata.get("auth"),"x-openb2c-audit":schema.metadata.get("audit"),"x-openb2c-integrations":schema.metadata.get("integrations"),"x-openb2c-effect-contracts":schema.metadata.get("effectContracts"),"x-openb2c-workflow":schema.metadata.get("presentation"),"x-openb2c-api-versioning":{"currentVersion":"0.1.0","requestHeader":"X-OpenB2C-API-Version","responseHeader":"X-OpenB2C-API-Version"}});
    if !schema.tables.contains_key("identity_registry") {
        doc["components"]["securitySchemes"]
            .as_object_mut()
            .unwrap()
            .remove("certificateAuth");
    }
    for (table, cols) in &schema.tables {
        if table.starts_with("identity_") {
            continue;
        }
        let entity = pascal(table);
        let mut props = Map::new();
        let mut required = Vec::new();
        for (field, col) in cols {
            if hidden(field, col) {
                continue;
            }
            props.insert(field.clone(), field_schema(field, col, true));
            if col.pk || col.required {
                required.push(field.clone());
            }
        }
        for (field, derived) in schema.derived.get(table).into_iter().flatten() {
            if derived.metadata["redact"] == true || derived.metadata["privacy"] == "secret" {
                continue;
            }
            let c = Column {
                kind: derived.kind,
                required: true,
                metadata: derived.metadata.clone(),
                ..Default::default()
            };
            let mut value = field_schema(field, &c, false);
            value["readOnly"] = json!(true);
            value["x-openb2c-derived"] = json!({"displayOnly":true,"dependencies":derived.dependencies,"template":derived.template,"expression":derived.expression});
            props.insert(field.clone(), value);
            required.push(field.clone());
        }
        doc["components"]["schemas"][&entity] = json!({"type":"object","properties":props,"required":required,"additionalProperties":false});
        doc["components"]["schemas"][format!("{entity}Row")] =
            json!({"$ref":format!("#/components/schemas/{entity}")});
        doc["components"]["schemas"][format!("{entity}Input")] = input_schema(schema, table, false);
        doc["components"]["schemas"][format!("{entity}Patch")] = input_schema(schema, table, true);
        let actions = vec![
            ("read".to_string(), "get", false),
            ("create".into(), "post", false),
            ("read".into(), "get", true),
            ("update".into(), "put", true),
            ("update".into(), "patch", true),
            ("delete".into(), "delete", true),
        ];
        let custom = schema
            .operations
            .get(table)
            .into_iter()
            .flatten()
            .filter(|(n, _)| !["read", "create", "update", "delete"].contains(&n.as_str()))
            .map(|(n, _)| (n.clone(), "post", true));
        for (action, method, item) in actions.into_iter().chain(custom) {
            if schema
                .operations
                .get(table)
                .and_then(|ops| ops.get(&action))
                .is_some_and(|op| !op.enabled)
            {
                continue;
            }
            let custom = !["read", "create", "update", "delete"].contains(&action.as_str());
            let path = if custom {
                format!("/api/{table}s/{{id}}/{}", action.replace('_', "-"))
            } else if item {
                format!("/api/{table}s/{{id}}")
            } else {
                format!("/api/{table}s")
            };
            let metadata = op_metadata(schema, table, &action);
            let mut entry = json!({"operationId":format!("{action}_{table}{}",if action=="read"&&!item{"s"}else{""}),"tags":[table],"summary":metadata["presentation"]["label"].as_str().unwrap_or(&action),"description":metadata["presentation"]["description"],"x-openb2c-operation":metadata,"x-openb2c-auth":metadata["policy"],"x-openb2c-audit":metadata["audit"],"x-openb2c-workflow":{"presentation":metadata["presentation"],"transitions":metadata["transitions"],"guard":metadata["guard"]},"responses":{}});
            for (code, description) in [
                ("200", "Success"),
                ("400", "Malformed request or unsupported version"),
                ("401", "Authentication required"),
                ("403", "Forbidden"),
                ("404", "Not found"),
                ("409", "State, constraint, or ETag conflict"),
                ("413", "Body too large"),
                ("415", "Unsupported content type"),
                ("422", "Invalid input"),
                ("429", "Rate limited"),
                ("504", "Timed out"),
            ] {
                entry["responses"][code] = json!({"description":description});
            }
            if action == "create" {
                entry["responses"]["201"] = json!({"description":"Created","content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{entity}")}}}});
            }
            if action == "read" {
                let response_schema = if item {
                    json!({"$ref":format!("#/components/schemas/{entity}")})
                } else {
                    json!({"type":"object","properties":{"items":{"type":"array","items":{"$ref":format!("#/components/schemas/{entity}")}},"total":{"type":"integer"},"limit":{"type":"integer"},"offset":{"type":"integer"}}})
                };
                entry["responses"]["200"]["content"] =
                    json!({"application/json":{"schema":response_schema}});
            }
            let mut params = vec![
                json!({"name":"X-OpenB2C-API-Version","in":"header","schema":{"type":"string","enum":["0.1.0"]}}),
            ];
            if item {
                params.push(json!({"name":"id","in":"path","required":true,"schema":{"type":if cols.values().find(|c|c.pk).is_some_and(|c|c.kind==SqlType::Integer){"integer"}else{"string"}}}));
            }
            if method != "get" && item {
                params.push(json!({"name":"If-Match","in":"header","schema":{"type":"string"}}));
            }
            if action == "read" && !item {
                for (name, kind) in [
                    ("limit", "integer"),
                    ("offset", "integer"),
                    ("sort", "string"),
                    ("order", "string"),
                ] {
                    params.push(json!({"name":name,"in":"query","schema":{"type":kind}}));
                }
            }
            entry["parameters"] = json!(params);
            if metadata["public"] != true {
                entry["security"] = if schema.tables.contains_key("identity_registry") {
                    json!([{ "bearerAuth": [] }, { "certificateAuth": [] }])
                } else {
                    json!([{ "bearerAuth": [] }])
                };
            }
            if ["create", "update"].contains(&action.as_str()) {
                entry["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{entity}{}",if action=="create"{"Input"}else{"Patch"})}}}});
            } else if custom {
                let operation = &schema.operations[table][&action];
                let properties: Map<String, Value> = operation
                    .set
                    .iter()
                    .filter(|(_, a)| a.input == Some(true))
                    .filter_map(|(f, _)| cols.get(f).map(|c| (f.clone(), field_schema(f, c, true))))
                    .collect();
                if !properties.is_empty() {
                    entry["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"type":"object","required":properties.keys().collect::<Vec<_>>(),"properties":properties,"additionalProperties":false}}}});
                }
            }
            if doc["paths"].get(&path).is_none() {
                doc["paths"][&path] = json!({});
            }
            doc["paths"][&path][method] = entry;
        }
    }
    for (path, method, public) in [
        ("/health", "get", true),
        ("/identity/public-key", "get", true),
        ("/identity/challenge", "post", true),
        ("/identity/verify", "post", true),
        ("/auth/context", "get", false),
        ("/auth/revoke-current", "post", false),
        ("/ops/effects", "get", false),
        ("/ops/effects/retry", "post", false),
        ("/ops/fake-emails", "get", false),
        ("/ops/metrics", "get", false),
    ] {
        if path.starts_with("/identity/") && !schema.tables.contains_key("identity_challenge") {
            continue;
        }
        doc["paths"][path] = json!({method:{"responses":{"200":{"description":"Success"}},"security":if public{json!([])}else{json!([{"bearerAuth":[]}])}}});
    }
    crate::capabilities::add_openapi(schema, &mut doc);
    doc
}
