use crate::*;

/// Collect every diagnostic before code generation touches its output directory.
pub fn validate(schema: &Schema) -> Vec<String> {
    let mut errors = Vec::new();
    if schema.tables.is_empty() {
        errors.push("tables: composition has no entities".into());
    }
    for (table, cols) in &schema.tables {
        if !identifier(table) {
            errors.push(format!("tables.{table}: invalid SQL identifier"));
        }
        if cols.values().filter(|c| c.pk).count() != 1 {
            errors.push(format!(
                "tables.{table}: exactly one primary key is required"
            ));
        }
        for (field, col) in cols {
            let path = format!("tables.{table}.{field}");
            validate_metadata(&col.metadata, &path, &mut errors);
            if let Some(pattern) = &col.constraints.pattern
                && regex::Regex::new(pattern).is_err()
            {
                errors.push(format!(
                    "{path}.constraints.pattern: invalid regular expression"
                ));
            }
            if col.relationship.is_some() && col.references.is_none() {
                errors.push(format!(
                    "{path}.relationship: relationship metadata requires a foreign key"
                ));
            }
            if !identifier(field) {
                errors.push(format!("{path}: invalid SQL identifier"));
            }
            if col.auto && (!col.pk || col.kind != SqlType::Integer) {
                errors.push(format!(
                    "{path}: autoincrement requires an integer primary key"
                ));
            }
            if let Some(value) = &col.references {
                match reference(value) {
                    Some((target, key)) => match schema.tables.get(target).and_then(|c| c.get(key))
                    {
                        Some(other) => {
                            if col.kind != other.kind {
                                errors
                                    .push(format!("{path}: foreign key type differs from {value}"));
                            }
                            let unique = schema.constraints.get(target).is_some_and(|g| {
                                g.unique.values().any(|c| {
                                    c.enforcement != Enforcement::Application
                                        && c.condition.is_none()
                                        && c.fields.len() == 1
                                        && c.fields[0].field == key
                                })
                            });
                            if !other.pk && !other.unique && !unique {
                                errors.push(format!(
                                    "{path}: target {value} must be primary or unique"
                                ));
                            }
                        }
                        None => errors
                            .push(format!("{path}: foreign key target {value} does not exist")),
                    },
                    None => errors.push(format!("{path}: invalid foreign key {value}")),
                }
            }
            for bounds in [&col.constraints.length, &col.constraints.range]
                .into_iter()
                .flatten()
            {
                if let (Some(min), Some(max)) = (bounds.min, bounds.max)
                    && min > max
                {
                    errors.push(format!("{path}: minimum exceeds maximum"));
                }
            }
            if let Some(bounds) = &col.constraints.length {
                for bound in [bounds.min, bounds.max].into_iter().flatten() {
                    if bound < 0.0 || bound.fract() != 0.0 {
                        errors.push(format!(
                            "{path}.constraints.length: bounds must be nonnegative integers"
                        ));
                    }
                }
            }
        }
    }
    for (table, groups) in &schema.constraints {
        for (kind, constraints) in [("unique", &groups.unique), ("check", &groups.check)] {
            for (name, constraint) in constraints {
                let path = format!("constraints.{table}.{kind}.{name}");
                if !schema.tables.contains_key(table) {
                    errors.push(format!("{path}: unknown entity"));
                }
                if constraint.fields.is_empty() {
                    errors.push(format!("{path}: fields must not be empty"));
                }
                for field in &constraint.fields {
                    validate_ref(schema, field, &path, &mut errors);
                }
                if kind == "check" && constraint.expression.is_none() {
                    errors.push(format!("{path}: check requires expression"));
                }
                for expr in [&constraint.expression, &constraint.condition]
                    .into_iter()
                    .flatten()
                {
                    validate_expr(schema, table, expr, &path, &mut errors);
                }
            }
        }
    }
    for (table, ops) in &schema.operations {
        for (name, op) in ops {
            let path = format!("operations.{table}.{name}");
            if !identifier(name) {
                errors.push(format!("{path}: invalid operation name"));
            }
            if !schema.tables.contains_key(table) {
                errors.push(format!("{path}: unknown entity"));
            }
            if let Some(expr) = &op.guard {
                validate_expr(schema, table, expr, &path, &mut errors);
            }
            validate_assignments(schema, table, &op.set, &path, &mut errors);
            for rel in &op.relationships {
                validate_ref(schema, &rel.field, &path, &mut errors);
                if rel.field.table != *table {
                    errors.push(format!(
                        "{path}.relationships: ownership fields must belong to {table}"
                    ));
                }
            }
            for transition in &op.transitions {
                validate_ref(schema, &transition.field, &path, &mut errors);
                if transition.field.table != *table || transition.from.is_empty() {
                    errors.push(format!(
                        "{path}.transitions: transition requires a local field and source states"
                    ));
                }
                if let Some(col) = schema
                    .tables
                    .get(table)
                    .and_then(|c| c.get(&transition.field.field))
                    && !col.constraints.values.is_empty()
                    && transition
                        .from
                        .iter()
                        .chain(std::iter::once(&transition.to))
                        .any(|v| !col.constraints.values.contains(v))
                {
                    errors.push(format!(
                        "{path}.transitions: state is outside the field enum"
                    ));
                }
                if op
                    .set
                    .get(&transition.field.field)
                    .and_then(|a| a.value.as_ref())
                    .is_some_and(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                            != transition.to
                    })
                {
                    errors.push(format!(
                        "{path}.transitions: destination differs from assignment"
                    ));
                }
            }
            for cascade in &op.cascade {
                validate_assignments(
                    schema,
                    &cascade.entity.table,
                    &cascade.set,
                    &path,
                    &mut errors,
                );
                if !schema.tables.contains_key(&cascade.entity.table) {
                    errors.push(format!("{path}.cascade: unknown target entity"));
                }
            }
            if let Some(group) = op.presentation["group"].as_str()
                && !schema
                    .metadata
                    .get("presentation")
                    .is_some_and(|p| p["groups"].get(group).is_some())
            {
                errors.push(format!("{path}.presentation.group: unknown group {group}"));
            }
            if let Some(severity) = op.presentation["confirmation"]["severity"].as_str()
                && !["info", "warning", "danger"].contains(&severity)
            {
                errors.push(format!(
                    "{path}.presentation.confirmation.severity: invalid severity"
                ));
            }
            for effect in &op.effects {
                if ["emit", "notify", "call"]
                    .iter()
                    .filter(|kind| !effect[**kind].is_null())
                    .count()
                    != 1
                {
                    errors.push(format!(
                        "{path}.effects: exactly one effect type is required"
                    ));
                }
                if let Some(contracts) = schema.metadata.get("effectContracts") {
                    if let Some(event) = effect["emit"]["event"].as_str()
                        && contracts["events"].get(event).is_none()
                    {
                        errors.push(format!("{path}.effects.emit: undeclared event {event}"));
                    }
                    if let Some(template) = effect["notify"]["template"]["template"].as_str()
                        && contracts["notifications"]["templates"]
                            .get(template)
                            .is_none()
                    {
                        errors.push(format!(
                            "{path}.effects.notify: undeclared template {template}"
                        ));
                    }
                    if let Some(recipient) = effect["notify"]["to"]["recipient"].as_str()
                        && contracts["notifications"]["recipients"]
                            .get(recipient)
                            .is_none()
                    {
                        errors.push(format!(
                            "{path}.effects.notify: undeclared recipient {recipient}"
                        ));
                    }
                    if let (Some(service), Some(action)) = (
                        effect["call"]["service"].as_str(),
                        effect["call"]["action"].as_str(),
                    ) && contracts["calls"][service].get(action).is_none()
                    {
                        errors.push(format!(
                            "{path}.effects.call: undeclared action {service}.{action}"
                        ));
                    }
                }
            }
        }
    }
    for (table, fields) in &schema.derived {
        for (name, derived) in fields {
            let path = format!("derived.{table}.{name}");
            validate_metadata(&derived.metadata, &path, &mut errors);
            if !identifier(name) || !schema.tables.contains_key(table) {
                errors.push(format!("{path}: invalid derived field"));
            }
            if schema
                .tables
                .get(table)
                .is_some_and(|c| c.contains_key(name))
            {
                errors.push(format!("{path}: shadows a stored field"));
            }
            if usize::from(derived.template.is_some()) + usize::from(derived.expression.is_some())
                != 1
            {
                errors.push(format!(
                    "{path}: exactly one template or expression is required"
                ));
            }
            for field in &derived.dependencies {
                validate_ref(schema, field, &path, &mut errors);
            }
            if let Some(expr) = &derived.expression {
                validate_expr(schema, table, expr, &path, &mut errors);
            }
        }
    }
    for (kind, seeds) in [
        ("reference", &schema.seed.reference),
        ("fixtures", &schema.seed.fixtures),
    ] {
        for (table, rows) in seeds {
            if !schema.tables.contains_key(table) {
                errors.push(format!("seed.{kind}.{table}: unknown entity"));
                continue;
            }
            for row in rows {
                for (field, value) in row {
                    if !schema.tables[table].contains_key(field) {
                        errors.push(format!("seed.{kind}.{table}.{field}: unknown field"));
                    }
                    if value.is_array() || value.is_object() {
                        errors.push(format!(
                            "seed.{kind}.{table}.{field}: expected scalar value"
                        ));
                    }
                }
            }
        }
    }
    if !schema.seed.catalog.is_empty() && !schema.capabilities.contains_key("catalog") {
        errors.push("demo.products requires the catalog capability".into());
    }
    let code_pattern = regex::Regex::new("^[a-z0-9][a-z0-9_-]*$").unwrap();
    for (code, product) in &schema.seed.catalog {
        let path = format!("demo.products.{code}");
        if code.is_empty() || code.len() > 100 || !code_pattern.is_match(code) {
            errors.push(format!("{path}: invalid product code"));
        }
        if product.name.is_empty() || product.name.chars().count() > 160 {
            errors.push(format!("{path}.name: expected 1 to 160 characters"));
        }
        if product.currency.len() != 3 || !product.currency.bytes().all(|b| b.is_ascii_uppercase())
        {
            errors.push(format!(
                "{path}.currency: expected a three-letter uppercase currency"
            ));
        }
        let mut prices = std::collections::BTreeSet::new();
        for price in &product.prices {
            if price.amount < 1 || !["one_off", "month", "year"].contains(&price.interval.as_str())
            {
                errors.push(format!("{path}.prices: expected a positive minor-unit amount and one_off, month or year interval"));
            }
            if !prices.insert((price.amount, &price.interval)) {
                errors.push(format!("{path}.prices: duplicate price"));
            }
        }
    }
    errors.extend(crate::capabilities::validate(schema));
    if schema
        .metadata
        .get("commerce")
        .is_some_and(|c| c["enabled"] == true)
    {
        errors.push("commerce.enabled is retired; select explicit capability modules".into());
    }
    errors
}

fn validate_metadata(metadata: &Value, path: &str, errors: &mut Vec<String>) {
    for field in ["label", "helpText", "placeholder", "format"] {
        if !metadata[field].is_null() && !metadata[field].is_string() {
            errors.push(format!("{path}.metadata.{field}: expected text"));
        }
    }
    if let Some(privacy) = metadata["privacy"].as_str()
        && !["public", "internal", "sensitive", "secret"].contains(&privacy)
    {
        errors.push(format!("{path}.metadata.privacy: invalid privacy"));
    }
    if !metadata["redact"].is_null() && !metadata["redact"].is_boolean() {
        errors.push(format!("{path}.metadata.redact: expected boolean"));
    }
    if !metadata["displayPriority"].is_null() && !metadata["displayPriority"].is_number() {
        errors.push(format!("{path}.metadata.displayPriority: expected number"));
    }
}

fn validate_ref(schema: &Schema, field: &FieldRef, path: &str, errors: &mut Vec<String>) {
    if schema
        .tables
        .get(&field.table)
        .and_then(|c| c.get(&field.field))
        .is_none()
    {
        errors.push(format!(
            "{path}: unknown field {}.{}",
            field.table, field.field
        ));
    }
}

fn validate_assignments(
    schema: &Schema,
    table: &str,
    assignments: &Assignments,
    path: &str,
    errors: &mut Vec<String>,
) {
    for (field, assignment) in assignments {
        if schema
            .tables
            .get(table)
            .and_then(|c| c.get(field))
            .is_none()
        {
            errors.push(format!("{path}: unknown assignment target {table}.{field}"));
        }
        if assignment.source_count() != 1 {
            errors.push(format!(
                "{path}.set.{field}: exactly one assignment source is required"
            ));
        }
        if let Some(expr) = &assignment.expression {
            validate_expr(schema, table, expr, path, errors);
        }
    }
}

fn validate_expr(schema: &Schema, table: &str, expr: &Expr, path: &str, errors: &mut Vec<String>) {
    match expr {
        Expr::Field { name } => {
            if schema.tables.get(table).and_then(|c| c.get(name)).is_none() {
                errors.push(format!("{path}: unknown field {table}.{name}"));
            }
        }
        Expr::Rel { entity, field, via } => {
            validate_ref(
                schema,
                &FieldRef {
                    table: entity.clone(),
                    field: field.clone(),
                    references: None,
                },
                path,
                errors,
            );
            let connected = via.is_some()
                || schema.tables.get(table).is_some_and(|cols| {
                    cols.values().any(|c| {
                        c.references
                            .as_deref()
                            .and_then(reference)
                            .is_some_and(|(target, _)| target == entity)
                    })
                });
            if !connected {
                errors.push(format!(
                    "{path}: no foreign key connects {table} to {entity}"
                ));
            }
        }
        Expr::Bin { op, left, right } => {
            if ![
                "==", "!=", "<", "<=", ">", ">=", "+", "-", "*", "/", "&&", "||",
            ]
            .contains(&op.as_str())
            {
                errors.push(format!("{path}: unsupported operator {op}"));
            }
            validate_expr(schema, table, left, path, errors);
            validate_expr(schema, table, right, path, errors);
        }
        Expr::Un { op, arg } => {
            if !["!", "isNull", "notNull", "-"].contains(&op.as_str()) {
                errors.push(format!("{path}: unsupported unary operator {op}"));
            }
            validate_expr(schema, table, arg, path, errors);
        }
        Expr::Agg {
            rel,
            op,
            field,
            pred,
        } => {
            let entity = rel["table"].as_str().or_else(|| rel["entity"].as_str());
            if !["count", "sum", "any", "all"].contains(&op.as_str()) {
                errors.push(format!("{path}: unsupported aggregate {op}"));
            }
            if let Some(entity) = entity.filter(|entity| schema.tables.contains_key(*entity)) {
                if let Some(field) = field {
                    validate_ref(
                        schema,
                        &FieldRef {
                            table: entity.into(),
                            field: field.clone(),
                            references: None,
                        },
                        path,
                        errors,
                    );
                }
                if let Some(pred) = pred {
                    validate_expr(schema, entity, pred, path, errors);
                }
            } else {
                errors.push(format!("{path}: aggregate references an unknown entity"));
            }
        }
        Expr::Lit { .. } => {}
    }
}
