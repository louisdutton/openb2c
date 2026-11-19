# Reusable business compositions. Capability modules select these reactions.
{
  config,
  lib,
}: let
  w = import ./workflow.nix;
  inherit (w) field value eq ne all and when branch select require update concat;
  table = cap: role: config.capabilities.${cap}.entities.${role};
  sameSale = record: all (map (name: eq (field "${record}.${name}") (field "event.${name}")) ["user_id" "amount_minor" "currency"]);
  status = state: eq (field "event.status") (value state);
  ref = prefix: record: concat [(value prefix) (field "${record}.id")];
  sourceOrder = ref "order:" "event";
  sourceSubscription = ref "subscription-payment:" "event";
  cashAccount = lib.optionalAttrs config.billing.payments.enable {cash_account = value "customer_deposits";};
  depositedInvoice = source: steps:
    [(select (table "billing" "invoice") "cash_invoice" {source_key = source;} [
      (select (table "billing" "receipt") "cash_receipt" {invoice_id = field "cash_invoice.id";} [
        (select (table "accounting" "line") "cash_line" {journal_id = field "cash_receipt.journal_id";} [
          (when (ne (field "cash_line.debit") (value 0)) [
            (select (table "accounting" "account") "cash_account" {id = field "cash_line.account_id"; code = value "customer_deposits";} steps)
          ])
        ])
      ])
    ])];
  cashCorrection = payment: steps:
    if config.billing.payments.enable then steps else
      lib.optionals (config.orders.enable && config.orders.payments.enable) [
        (select (table "order_payments" "link") "cash_link" {payment_id = payment;}
          (depositedInvoice (concat [(value "order:") (field "cash_link.order_id")]) steps))
      ] ++ lib.optionals config.subscriptions.enable
        (depositedInvoice (concat [(value "subscription-payment:") payment]) steps);
  billingRefund = source: enabled:
    if enabled
    then [((w.refund source) // cashAccount)]
    else [
      (select (table "billing" "invoice") "invoice" {source_key = source;} [((w.refund source) // cashAccount)])
    ];
in {
  reversedCash = {
    event = "bank_transaction";
    order = -10;
    steps = [
      (when (eq (field "event.status") (value "reversed")) [
        (select (table "payments" "allocation") "allocation" {transaction_id = field "event.id"; kind = value "receipt";} (cashCorrection (field "allocation.payment_id") [
          (w.cash (ref "allocation-reversal:" "allocation") (field "allocation.amount_minor") (field "event.currency") (value "customer_deposits") (value "bank"))
        ]))
      ])
    ];
  };
  allocatedCash = {
    event = "payment_allocation";
    steps = [
      (select (table "payments" "payment") "request" {id = field "event.payment_id";} [
        (branch (eq (field "event.kind") (value "receipt")) (lib.optionals config.billing.payments.enable [
          (w.cash (ref "allocation:" "event") (field "event.amount_minor") (field "request.currency") (value "bank") (value "customer_deposits"))
        ]) (cashCorrection (field "event.payment_id") [
          (w.cash (ref "allocation:" "event") (field "event.amount_minor") (field "request.currency") (value "customer_deposits") (value "bank"))
        ]))
      ])
    ];
  };
  transferMatching = {
    event = "bank_transaction";
    steps = [
      (when (eq (field "event.status") (value "reversed")) [{
        _t = "reverse_transfer";
        transaction = field "event.id";
      }])
      (when (all [
        (eq (field "event.direction") (value "in"))
        (eq (field "event.status") (value "settled"))
      ]) [
        (select (table "payments" "payment") "request" {
          bank_reference = field "event.reference";
          bank_account = field "event.account";
          currency = field "event.currency";
          amount_minor = field "event.amount_minor";
          status = value "pending";
        } [{
          _t = "allocate_transfer";
          transaction = field "event.id";
          payment = field "request.id";
          amount = field "event.amount_minor";
        }])
      ])
    ];
  };
  orderPayment = {
    event = "payment";
    order = 10;
    steps = [
      (select (table "order_payments" "link") "link" {payment_id = field "event.id";} [
        (select (table "orders" "order") "order" {id = field "link.order_id";} [
          (require (sameSale "order") "Payment does not match its order")
          (branch (status "succeeded") [
              (require (eq (field "order.status") (value "pending")) "Payment conflicts with its order state")
              (update "order" {status = value "paid";} "orders.payment" "order")
            ] [
              (branch (status "refunded") [
                (require (eq (field "order.status") (value "paid")) "Payment conflicts with its order state")
                (update "order" {status = value "refunded";} "orders.payment" "order")
              ] [(require (status "failed") "Payment conflicts with its order state")])
            ])
        ])
      ])
    ];
  };
  orderCancellation = {
    event = "order";
    order = 10;
    steps = [
      (when (status "cancelled") [
        (select (table "order_payments" "link") "link" {order_id = field "event.id";} [(w.cancelPayment (field "link.payment_id"))])
      ])
    ];
  };
  purchasedAccess = {
    event = "order";
    order = 30;
    steps = [
      (select (table "orders" "item") "item" {order_id = field "event.id";} (let
        source = concat [(value "order:") (field "event.id") (value ":item:") (field "item.id")];
      in
        lib.optionals config.orders.entitlements.enable [
          (when (status "paid") [(w.grant (field "event.user_id") (field "item.product_code") source null)])
        ]
        ++ [
          (when (w.or (status "refunded") (status "cancelled")) [(w.revoke source)])
        ]))
    ];
  };
  subscriptionPayment = {
    event = "payment";
    order = 20;
    steps = [
      (select (table "subscriptions" "attempt") "attempt" {payment_id = field "event.id";} [
        (select (table "subscriptions" "subscription") "subscription" {id = field "attempt.subscription_id";} [
          (require (sameSale "subscription") "Payment does not match its subscription")
          (when (status "succeeded") [
            (require (and (ne (field "subscription.status") (value "cancelled")) (eq (field "attempt.cycle") (w.next (field "subscription.cycle")))) "Subscription no longer accepts this billing cycle")
            (w.schedule "subscription" (field "attempt.cycle") "period" [
              (update "subscription" {
                status = value "active";
                cycle = field "attempt.cycle";
                started_at = field "period.anchor";
                current_period_start = field "period.start";
                current_period_end = field "period.end";
              } "subscriptions.payment" "subscription")
            ])
          ])
          (when (and (status "failed") (ne (field "subscription.status") (value "cancelled"))) [
            (update "subscription" {status = value "past_due";} "subscriptions.payment" "subscription")
          ])
          (when (status "refunded") [
            (w.cancelSubscriptionPayments (field "subscription.id"))
            (update "subscription" {
              status = value "cancelled";
              cancel_at_period_end = value 0;
            } "subscriptions.payment" "subscription")
          ])
        ])
      ])
    ];
  };
  subscriptionDue = {
    event = "subscription_due";
    steps = [
      (branch (eq (field "event.cancel_at_period_end") (value 1)) [
        (w.cancelSubscriptionPayments (field "event.id"))
        (update "event" {status = value "cancelled"; cancel_at_period_end = value 0;} "subscriptions.expire" "subscription")
      ] [
        (update "event" {status = value "past_due";} "subscriptions.await_transfer" "subscription")
      ])
    ];
  };
  subscriptionAccess = {
    event = "subscription";
    order = 30;
    steps = [
      (branch (status "active") (lib.optionals config.subscriptions.entitlements.enable [
        (w.grant (field "event.user_id") (field "event.product_code") (ref "subscription:" "event") (field "event.current_period_end"))
      ]) [(w.revoke (ref "subscription:" "event"))])
    ];
  };
  orderBilling = {
    event = "order";
    order = 40;
    steps =
      lib.optionals config.billing.orders.enable [
        (when (status "paid") [((w.sale "event" sourceOrder (concat [(value "Order ") (field "event.id")]) null null) // cashAccount)])
      ]
      ++ [(when (status "refunded") (billingRefund sourceOrder config.billing.orders.enable))];
  };
  subscriptionBilling = {
    event = "payment";
    order = 50;
    steps = [
      (select (table "subscriptions" "attempt") "attempt" {payment_id = field "event.id";} (
        lib.optionals config.billing.subscriptions.enable [
          (when (status "succeeded") [
            (select (table "subscriptions" "subscription") "subscription" {id = field "attempt.subscription_id";} [
              ((w.sale "event" sourceSubscription (concat [(value "Subscription ") (field "attempt.subscription_id") (value " cycle ") (field "attempt.cycle")]) (w.date (field "subscription.current_period_start")) (w.date (field "subscription.current_period_end"))) // cashAccount)
            ])
          ])
        ]
        ++ [(when (status "refunded") (billingRefund sourceSubscription config.billing.subscriptions.enable))]
      ))
    ];
  };
}
