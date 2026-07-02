# Expression AST builders
# Usage: E.eq (E.f config.refs.ticket.status) (E.lit config.values.ticket.status.pending)
# Exports to JSON AST that codegen compiles to target language

let
  fieldName = ref: if builtins.isAttrs ref && ref ? field then ref.field else ref;
  tableName = ref: if builtins.isAttrs ref && ref ? table then ref.table else ref;
in {
  # Field reference (local entity)
  f = ref: { _t = "field"; name = fieldName ref; };
  field = ref: { _t = "field"; name = fieldName ref; };

  # Related entity field (via FK)
  rel = entity: ref: { _t = "rel"; entity = tableName entity; field = fieldName ref; };

  # Related entity through junction table
  relVia = entity: via: ref: {
    _t = "rel";
    entity = tableName entity;
    via = tableName via;
    field = fieldName ref;
  };

  # Literal value
  lit = value: { _t = "lit"; inherit value; };

  # Comparison operators
  eq = left: right: { _t = "bin"; op = "=="; inherit left right; };
  ne = left: right: { _t = "bin"; op = "!="; inherit left right; };
  lt = left: right: { _t = "bin"; op = "<"; inherit left right; };
  lte = left: right: { _t = "bin"; op = "<="; inherit left right; };
  gt = left: right: { _t = "bin"; op = ">"; inherit left right; };
  gte = left: right: { _t = "bin"; op = ">="; inherit left right; };

  # Boolean operators
  and = left: right: { _t = "bin"; op = "&&"; inherit left right; };
  or = left: right: { _t = "bin"; op = "||"; inherit left right; };
  not = arg: { _t = "un"; op = "!"; inherit arg; };

  # Null checks
  isNull = arg: { _t = "un"; op = "isNull"; inherit arg; };
  notNull = arg: { _t = "un"; op = "notNull"; inherit arg; };

  # Arithmetic
  add = left: right: { _t = "bin"; op = "+"; inherit left right; };
  sub = left: right: { _t = "bin"; op = "-"; inherit left right; };
  mul = left: right: { _t = "bin"; op = "*"; inherit left right; };
  div = left: right: { _t = "bin"; op = "/"; inherit left right; };

  # Aggregations on related collections
  count = rel: { _t = "agg"; op = "count"; inherit rel; };
  sum = rel: ref: { _t = "agg"; op = "sum"; inherit rel; field = fieldName ref; };
  any = rel: pred: { _t = "agg"; op = "any"; inherit rel pred; };
  all = rel: pred: { _t = "agg"; op = "all"; inherit rel pred; };

  # Convenience: always true/false
  true_ = { _t = "lit"; value = true; };
  false_ = { _t = "lit"; value = false; };
}
