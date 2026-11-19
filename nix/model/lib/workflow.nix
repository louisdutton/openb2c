# Business wiring constructors. The compiler validates types, fields and calls.
rec {
  field = path: {inherit path;};
  value = value: {inherit value;};
  concat = parts: {inherit parts;};
  next = term: {next = term;};
  date = term: {date = term;};
  eq = left: right: {
    op = "eq";
    inherit left right;
  };
  ne = left: right: {
    op = "ne";
    inherit left right;
  };
  and = left: right: {
    op = "and";
    inherit left right;
  };
  or = left: right: {
    op = "or";
    inherit left right;
  };
  all = conditions: builtins.foldl' and (value true) conditions;
  when = condition: steps: {
    _t = "if";
    inherit condition steps;
    otherwise = [];
  };
  branch = condition: steps: otherwise: {
    _t = "if";
    inherit condition steps otherwise;
  };
  select = table: bind: where: steps: {
    _t = "select";
    inherit table bind where steps;
  };
  require = condition: message: {
    _t = "require";
    inherit condition message;
  };
  update = record: set: action: event: {
    _t = "update";
    inherit record set action event;
  };
  grant = user: resource: source: end: {
    _t = "grant";
    inherit user resource source end;
  };
  revoke = source: {
    _t = "revoke";
    inherit source;
  };
  cancelPayment = id: {
    _t = "cancel_payment";
    inherit id;
  };
  cancelSubscriptionPayments = id: {
    _t = "cancel_subscription_payments";
    inherit id;
  };
  schedule = record: cycle: bind: steps: {
    _t = "schedule";
    inherit record cycle bind steps;
  };
  sale = record: source: description: start: end: {
    _t = "sale";
    inherit record source description start end;
  };
  cash = source: amount: currency: debit: credit: {
    _t = "cash"; inherit source amount currency debit credit;
  };
  refund = source: {
    _t = "refund";
    inherit source;
  };
}
