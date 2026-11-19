//! Build-time workflow syntax. Generated services execute compiled Rust, not this AST.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub event: String,
    #[serde(default)]
    pub order: i32,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Term {
    Field {
        path: String,
    },
    Literal {
        value: Value,
    },
    Concat {
        parts: Vec<Term>,
    },
    Next {
        next: Box<Term>,
    },
    Date {
        date: Box<Term>,
    },
    Binary {
        op: String,
        left: Box<Term>,
        right: Box<Term>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "_t", deny_unknown_fields)]
pub enum Step {
    #[serde(rename = "sync_transfers")]
    SyncTransfers,
    #[serde(rename = "reconcile_transfers")]
    ReconcileTransfers,
    #[serde(rename = "reverse_transfer")]
    ReverseTransfer { transaction: Term },
    #[serde(rename = "allocate_transfer")]
    AllocateTransfer {
        transaction: Term,
        payment: Term,
        amount: Term,
    },
    #[serde(rename = "renew_subscriptions")]
    RenewSubscriptions,
    #[serde(rename = "select")]
    Select {
        table: String,
        bind: String,
        r#where: BTreeMap<String, Term>,
        steps: Vec<Step>,
    },
    #[serde(rename = "if")]
    If {
        condition: Term,
        steps: Vec<Step>,
        #[serde(default)]
        otherwise: Vec<Step>,
    },
    #[serde(rename = "require")]
    Require { condition: Term, message: String },
    #[serde(rename = "update")]
    Update {
        record: String,
        set: BTreeMap<String, Term>,
        action: String,
        event: String,
    },
    #[serde(rename = "grant")]
    Grant {
        user: Term,
        resource: Term,
        source: Term,
        end: Option<Term>,
    },
    #[serde(rename = "cash")]
    Cash {
        source: Term,
        amount: Term,
        currency: Term,
        debit: Term,
        credit: Term,
    },
    #[serde(rename = "revoke")]
    Revoke { source: Term },
    #[serde(rename = "cancel_payment")]
    CancelPayment { id: Term },
    #[serde(rename = "cancel_subscription_payments")]
    CancelSubscriptionPayments { id: Term },
    #[serde(rename = "schedule")]
    Schedule {
        record: String,
        cycle: Term,
        bind: String,
        steps: Vec<Step>,
    },
    #[serde(rename = "sale")]
    Sale {
        record: String,
        source: Term,
        description: Term,
        start: Option<Term>,
        end: Option<Term>,
        cash_account: Option<Term>,
    },
    #[serde(rename = "refund")]
    Refund {
        source: Term,
        cash_account: Option<Term>,
    },
}
