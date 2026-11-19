use openb2c_runtime::{AuthContext,Service};
use serde_json::{json,Value};

fn service()->Service{Service::open(serde_json::from_str(include_str!("../app/schema.json")).unwrap(),":memory:",true).unwrap()}
fn id(v:&Value)->i64{v["id"].as_i64().unwrap()}
fn customer(s:&Service)->AuthContext{let user=s.ensure_customer("reader@example.test",&AuthContext::system()).unwrap();AuthContext{user_id:Some(user),scopes:s.self_scopes()}}
fn payment(s:&Service,a:&AuthContext,key:&str)->Value{
 let order=s.b2c_create_order(&json!({"idempotency_key":key,"items":[{"price_id":1,"quantity":1}]}),a).unwrap();
 s.b2c_order_payment(id(&order["order"]),&json!({"idempotency_key":key}),a).unwrap()
}
fn fact(p:&Value,key:&str,amount:i64)->Value{json!({"transaction_id":key,"account":"simulated","amount_minor":amount,"currency":"GBP","direction":"in","status":"settled","reference":p["bank_reference"],"settled_at":"2025-01-01T12:00:00+00:00","updated_at":"2025-01-01T12:00:00+00:00"})}
fn import(s:&Service,v:&Value)->Value{s.import_simulated_transfer(v,&AuthContext::system()).unwrap()}
fn allocate(s:&Service,t:&Value,p:&Value,amount:i64)->Value{s.allocate_transfer(id(t),&json!({"payment_id":id(p),"amount_minor":amount}),&AuthContext::system()).unwrap()}
fn access(s:&Service,a:&AuthContext)->usize{s.b2c_entitlements(a).unwrap()["items"].as_array().unwrap().len()}

#[test]
fn exact_receipts_fulfil_once_and_never_trust_customer_confirmation(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"exact");
 assert_eq!(p["provider"],"bank_transfer");assert_eq!(p["instructions"]["simulated"],true);
 assert_eq!(p["bank_reference"].as_str().unwrap().len(),17);
 assert!(p["instructions"]["qr_svg"].as_str().unwrap().contains("<svg"));
 let input=fact(&p,"bank-1",1500);
 assert!(s.import_simulated_transfer(&input,&a).is_err());assert!(s.bank_transfers(&a).is_err());
 assert!(s.create("bank_transaction",&input,&AuthContext::system()).is_err());
 let t=import(&s,&input);assert_eq!(t["unallocated_minor"],0);assert_eq!(access(&s,&a),1);
 assert_eq!(s.b2c_payment(id(&p),&a).unwrap()["status"],"succeeded");
 assert_eq!(import(&s,&input),t);assert_eq!(access(&s,&a),1);
 assert_eq!(s.backoffice_list("billing","invoice",&AuthContext::system()).unwrap()["items"].as_array().unwrap().len(),1);
 assert_eq!(s.db.query_row("SELECT coalesce(sum(l.debit-l.credit),0) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='bank'",[],|r|r.get::<_,i64>(0)).unwrap(),1500);
 assert_eq!(s.db.query_row("SELECT coalesce(sum(l.debit-l.credit),0) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id WHERE a.code='clearing'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
 let mut conflict=input.clone();conflict["amount_minor"]=json!(1600);assert!(s.import_simulated_transfer(&conflict,&AuthContext::system()).is_err());
 let duplicate=import(&s,&fact(&p,"bank-2",1500));assert_eq!(duplicate["unallocated_minor"],1500);
 assert!(s.allocate_transfer(id(&duplicate),&json!({"payment_id":id(&p),"amount_minor":1500}),&AuthContext::system()).is_err());
 assert!(s.b2c_payment(id(&p),&AuthContext{user_id:Some(999),scopes:s.self_scopes()}).is_err());
}
#[test]
fn pending_wrong_reference_and_partial_receipts_require_review(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"partial");
 let mut v=fact(&p,"partial-1",700);v["reference"]=json!("MISSING");v["status"]=json!("pending");v["settled_at"]=Value::Null;
 let t=import(&s,&v);assert_eq!(access(&s,&a),0);
 assert!(s.allocate_transfer(id(&t),&json!({"payment_id":id(&p),"amount_minor":700}),&AuthContext::system()).is_err());
 v["status"]=json!("settled");v["settled_at"]=json!("2025-01-01T12:01:00+00:00");v["updated_at"]=v["settled_at"].clone();
 let t=import(&s,&v);assert_eq!(t["unallocated_minor"],700);
 assert!(s.allocate_transfer(id(&t),&json!({"payment_id":id(&p),"amount_minor":700}),&a).is_err());
 let allocated=allocate(&s,&t,&p,700);assert_eq!(allocated["unallocated_minor"],0);
 assert_eq!(s.b2c_payment(id(&p),&a).unwrap()["status"],"pending");assert_eq!(access(&s,&a),0);
 assert_eq!(allocate(&s,&t,&p,700),allocated);
 let second=import(&s,&fact(&p,"partial-2",800));assert_eq!(second["unallocated_minor"],800);
 allocate(&s,&second,&p,800);assert_eq!(access(&s,&a),1);
 assert_eq!(s.bank_transfers(&AuthContext::system()).unwrap()["unresolved_count"],0);
 let mut invalid=fact(&p,"wrong-account",1500);invalid["account"]=json!("someone-else");assert!(s.import_simulated_transfer(&invalid,&AuthContext::system()).is_err());
}
#[test]
fn overpayments_cancellation_and_refunds_preserve_actual_cash_facts(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"overpaid");
 let t=import(&s,&fact(&p,"overpaid",2000));assert_eq!(t["unallocated_minor"],2000);assert_eq!(access(&s,&a),0);
 let t=allocate(&s,&t,&p,1500);assert_eq!(t["unallocated_minor"],500);assert_eq!(access(&s,&a),1);
 // A credit or a cancelled access grant cannot invent a refund. Import a separate
 // settled outgoing transfer, then explicitly allocate it to the original request.
 let mut refund=fact(&p,"refund",1500);refund["direction"]=json!("out");let outgoing=import(&s,&refund);
 assert_eq!(s.b2c_payment(id(&p),&a).unwrap()["status"],"succeeded");
 allocate(&s,&outgoing,&p,1500);assert_eq!(access(&s,&a),0);assert_eq!(s.b2c_payment(id(&p),&a).unwrap()["status"],"refunded");
 let p2=payment(&s,&a,"cancelled");s.b2c_cancel_order(2,&a).unwrap();let late=import(&s,&fact(&p2,"late",1500));
 assert_eq!(late["unallocated_minor"],1500);assert_eq!(access(&s,&a),0);
}
#[test]
fn a_confirmed_full_bank_reversal_revokes_access_and_is_replay_safe(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"reverse");let mut v=fact(&p,"reversed-bank-id",1500);
 import(&s,&v);assert_eq!(access(&s,&a),1);
 v["status"]=json!("reversed");v["updated_at"]=json!("2025-01-02T12:00:00+00:00");
 let t=import(&s,&v);assert!(t["review_reason"].is_null(),"{t}");assert_eq!(access(&s,&a),0);
 assert_eq!(s.b2c_payment(id(&p),&a).unwrap()["status"],"refunded");assert_eq!(import(&s,&v),t);
 assert_eq!(s.backoffice_list("billing","credit",&AuthContext::system()).unwrap()["items"].as_array().unwrap().len(),1);
}
#[test]
fn subscriptions_keep_a_standing_order_reference_and_renew_only_after_receipt(){
 let s=service();let a=customer(&s);let sub=s.b2c_create_subscription(&json!({"idempotency_key":"member","price_id":2}),&a).unwrap();
 let first=&sub["payment"];import(&s,&fact(first,"month-1",500));
 let active=s.b2c_subscription(id(&sub["subscription"]),&a).unwrap();assert_eq!(active["subscription"]["status"],"active");
 let at=serde_json::from_value(active["subscription"]["current_period_end"].clone()).unwrap();
 let next=s.b2c_renew_due_at(&AuthContext::system(),at).unwrap();let next=&next["payments"][0];
 assert_eq!(next["bank_reference"],first["bank_reference"]);assert_ne!(next["reference"],first["reference"]);
 assert_eq!(s.b2c_subscription(1,&a).unwrap()["subscription"]["cycle"],0);
 assert_eq!(s.b2c_subscription(1,&a).unwrap()["subscription"]["status"],"past_due");
 let mut receipt=fact(next,"month-2",500);receipt["settled_at"]=json!("2025-02-01T12:00:00+00:00");receipt["updated_at"]=receipt["settled_at"].clone();import(&s,&receipt);
 assert_eq!(s.b2c_subscription(1,&a).unwrap()["subscription"]["cycle"],1);
 s.b2c_cancel_subscription(1,&json!({"at_period_end":false}),&a).unwrap();let late=import(&s,&fact(next,"after-cancel",500));assert_eq!(late["unallocated_minor"],500);
}
#[test]
fn unrelated_funds_require_an_audited_operator_disposition(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"review");let mut v=fact(&p,"unrelated",2500);v["reference"]=json!("OWNER CONTRIBUTION");let t=import(&s,&v);
 assert_eq!(s.bank_transfers(&AuthContext::system()).unwrap()["unresolved_count"],1);
 let input=json!({"exclude":true,"reason":"Owner contribution classified with a separate journal"});
 assert!(s.review_transfer(id(&t),&input,&a).is_err());
 let reviewed=s.review_transfer(id(&t),&input,&AuthContext::system()).unwrap();assert_eq!(reviewed["disposition"],"excluded");assert_eq!(reviewed["unallocated_minor"],2500);
 assert_eq!(s.bank_transfers(&AuthContext::system()).unwrap()["unresolved_count"],0);
 assert!(s.allocate_transfer(id(&t),&json!({"payment_id":id(&p),"amount_minor":1500}),&AuthContext::system()).is_err());
 assert_eq!(s.db.query_row("SELECT count(*) FROM bank_transaction",[],|r|r.get::<_,i64>(0)).unwrap(),1);
}

fn balance(s:&Service,code:&str,through:&str)->i64 {
 s.db.query_row("SELECT coalesce(sum(l.debit-l.credit),0) FROM journal_line l JOIN ledger_account a ON a.id=l.account_id JOIN journal j ON j.id=l.journal_id WHERE a.code=? AND j.date<=?",[code,through],|r|r.get(0)).unwrap()
}
fn dated_fact(p:&Value,key:&str,amount:i64,day:&str)->Value {
 let mut v=fact(p,key,amount);v["settled_at"]=json!(format!("{day}T12:00:00+00:00"));v["updated_at"]=v["settled_at"].clone();v
}
#[test]
fn allocated_partial_cash_preserves_each_bank_date_without_duplicate_invoice_cash(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"dated-partial");
 let first=import(&s,&dated_fact(&p,"dated-first",700,"2025-01-01"));
 allocate(&s,&first,&p,700);
 assert_eq!(balance(&s,"bank","2025-01-01"),700);
 assert_eq!(balance(&s,"customer_deposits","2025-01-01"),-700);
 assert_eq!(s.b2c_payment(id(&p),&a).unwrap()["status"],"pending");assert_eq!(access(&s,&a),0);
 assert_eq!(s.backoffice_list("billing","invoice",&AuthContext::system()).unwrap()["items"],json!([]));
 let second=import(&s,&dated_fact(&p,"dated-second",800,"2025-01-02"));
 allocate(&s,&second,&p,800);
 assert_eq!(balance(&s,"bank","2025-01-01"),700);
 assert_eq!(balance(&s,"bank","2025-01-02"),1500);
 assert_eq!(balance(&s,"customer_deposits","2025-01-02"),0);
 assert_eq!(balance(&s,"receivables","2025-01-02"),0);
 assert_eq!(s.backoffice_invoice_view(1,&AuthContext::system()).unwrap()["date"],"2025-01-02");
 assert_eq!(s.bank_transfers(&AuthContext::system()).unwrap()["unresolved_count"],0);
 let count:i64=s.db.query_row("SELECT count(*) FROM journal",[],|r|r.get(0)).unwrap();
 allocate(&s,&second,&p,800);allocate(&s,&first,&p,700);
 assert_eq!(s.db.query_row("SELECT count(*) FROM journal",[],|r|r.get::<_,i64>(0)).unwrap(),count);
 assert_eq!(access(&s,&a),1);
 let mut refund=dated_fact(&p,"dated-refund",1500,"2025-01-03");refund["direction"]=json!("out");let r=import(&s,&refund);allocate(&s,&r,&p,1500);
 assert_eq!(balance(&s,"bank","2025-01-03"),0);assert_eq!(balance(&s,"customer_deposits","2025-01-03"),0);assert_eq!(access(&s,&a),0);
}
#[test]
fn allocation_order_cannot_backdate_fulfilment_before_the_latest_receipt(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"out-of-order");
 let later=import(&s,&dated_fact(&p,"later",800,"2025-01-02"));allocate(&s,&later,&p,800);
 let earlier=import(&s,&dated_fact(&p,"earlier",700,"2025-01-01"));allocate(&s,&earlier,&p,700);
 assert_eq!(s.backoffice_invoice_view(1,&AuthContext::system()).unwrap()["date"],"2025-01-02");
 assert_eq!(balance(&s,"bank","2025-01-01"),700);assert_eq!(balance(&s,"bank","2025-01-02"),1500);
}
#[test]
fn closed_period_partial_cash_is_durable_and_posts_once_in_an_open_period(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"closed-partial");let owner=AuthContext::system();
 s.backoffice_close(&json!({"through_date":"2025-01-01"}),&owner).unwrap();
 let first=import(&s,&dated_fact(&p,"closed-first",700,"2025-01-01"));allocate(&s,&first,&p,700);
 assert_eq!(balance(&s,"bank","2025-01-01"),0);
 let events=s.backoffice_list("billing","event",&owner).unwrap();assert_eq!(events["items"][0]["status"],"pending");assert!(events["items"][0]["occurred_at"].as_str().unwrap().starts_with("2025-01-01"));
 let second=import(&s,&dated_fact(&p,"open-second",800,"2025-01-02"));allocate(&s,&second,&p,800);assert_eq!(access(&s,&a),1);
 assert!(s.backoffice_post_billing_events(&json!({"date":"2025-01-01"}),&owner).is_err());
 assert_eq!(s.backoffice_post_billing_events(&json!({"date":"2025-01-03"}),&owner).unwrap()["posted"],1);
 assert_eq!(balance(&s,"bank","2025-01-03"),1500);assert_eq!(balance(&s,"customer_deposits","2025-01-03"),0);
 assert_eq!(s.backoffice_post_billing_events(&json!({"date":"2025-01-03"}),&owner).unwrap()["posted"],0);
}
#[test]
fn vat_drafts_flag_customer_deposits_across_reporting_boundaries(){
 let s=service();let a=customer(&s);let p=payment(&s,&a,"vat-deposit");let owner=AuthContext::system();
 let first=import(&s,&dated_fact(&p,"vat-first",700,"2025-01-01"));allocate(&s,&first,&p,700);
 let second=import(&s,&dated_fact(&p,"vat-second",800,"2025-01-02"));allocate(&s,&second,&p,800);
 for day in ["2025-01-01","2025-01-02"]{
  let report=s.backoffice_vat_report(&json!({"from":day,"through":day}),&owner).unwrap();
  assert!(report["blockers"].as_array().unwrap().iter().any(|b|b.to_string().contains("Customer deposits cross")),"{report}");
 }
}
