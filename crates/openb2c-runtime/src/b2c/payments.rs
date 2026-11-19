//! Customer-initiated transfers. There is deliberately no payment initiation API.
use super::*;
use crate::records::{
    PaymentsPayment as Payment, PaymentsPaymentProvider, PaymentsPaymentStatus as Status,
    PaymentsPaymentWrite as PaymentWrite, Record,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePayment {
    idempotency_key: String,
    amount_minor: i64,
    currency: String,
}

pub(super) fn reference() -> Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| invalid("Random source unavailable"))?;
    Ok(format!("P{}", hex::encode_upper(bytes)))
}
impl Service {
    pub fn b2c_pending_payments(&self, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.reconcile", auth)?;
        let items = Payment::select(self, "WHERE status='pending' ORDER BY id LIMIT 100", &[])?
            .iter()
            .map(|r| self.payment_view(r))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"items":items}))
    }
    pub(super) fn cap_payment(
        &self,
        user: i64,
        amount: i64,
        currency: &str,
        request_key: &str,
        auth: &AuthContext,
        now: DateTime<Utc>,
    ) -> Result<Payment> {
        if !(1..=1_000_000_000_000).contains(&amount) || currency != "GBP" {
            return Err(invalid(
                "Bank transfer requests require a positive GBP amount within range",
            ));
        }
        if let Some(existing) = Payment::first(
            self,
            "WHERE user_id=? AND request_key=?",
            &[&user, &request_key],
        )? {
            if existing.amount_minor != amount || existing.currency != currency {
                return Err(ServiceError::new(
                    "conflict",
                    "Payment key has different amount or currency",
                ));
            }
            return Ok(existing);
        }
        let destination = self.transfer_destination()?;
        let reference = reference()?;
        let row = PaymentWrite {
            user_id: user.into(),
            amount_minor: amount.into(),
            currency: currency.to_owned().into(),
            provider: PaymentsPaymentProvider::BankTransfer.into(),
            reference: reference.clone().into(),
            bank_reference: reference.into(),
            bank_account: destination.account.into(),
            beneficiary: destination.name.into(),
            sort_code: destination.sort_code.into(),
            account_number: destination.account_number.into(),
            status: Status::Pending.into(),
            request_key: request_key.to_owned().into(),
            created_at: now.to_rfc3339().into(),
            ..Default::default()
        }
        .insert(self)?;
        row.audit(self, "payments.request", auth, None)?;
        Ok(row)
    }
    pub(super) fn payment_view(&self, row: &Payment) -> Result<Value> {
        let mut value = row.wire(self)?;
        value["instructions"] = json!({"method":"bank_transfer","account_name":row.beneficiary,"sort_code":row.sort_code,"account_number":row.account_number,"reference":row.bank_reference,"amount_minor":row.amount_minor,"currency":row.currency,"simulated":self.cap_setting("payments","feed")==Some("simulated")});
        // This QR encodes readable instructions, not a universal banking-app URI.
        let payload = format!(
            "Bank transfer\nPayee: {}\nSort code: {}\nAccount: {}\nAmount: GBP {}.{:02}\nReference: {}",
            row.beneficiary,
            row.sort_code,
            row.account_number,
            row.amount_minor / 100,
            row.amount_minor % 100,
            row.bank_reference
        );
        let code = qrcode::QrCode::new(payload.as_bytes())
            .map_err(|_| invalid("Payment instructions exceed QR capacity"))?;
        value["instructions"]["qr_payload"] = json!(payload);
        value["instructions"]["qr_svg"] = json!(
            code.render::<qrcode::render::svg::Color>()
                .min_dimensions(256, 256)
                .build()
        );
        Ok(value)
    }
    pub fn b2c_create_payment(&self, input: &Value, auth: &AuthContext) -> Result<Value> {
        self.cap_command("payments", "payments.create", auth)?;
        let user = self.cap_customer(auth)?;
        let input: CreatePayment = decode(input)?;
        key(&input.idempotency_key)?;
        let tx = crate::service::transaction(&self.db)?;
        let row = self.cap_payment(
            user,
            input.amount_minor,
            &input.currency,
            &format!("standalone:{}", input.idempotency_key),
            auth,
            Utc::now(),
        )?;
        let response = self.payment_view(&row)?;
        crate::service::commit(tx)?;
        Ok(response)
    }
    pub fn b2c_payment(&self, id: i64, auth: &AuthContext) -> Result<Value> {
        self.payment_view(&self.owned::<Payment>(id, "payments", "payments.read", auth)?)
    }
    pub(crate) fn cap_cancel_payment(&self, id: i64, auth: &AuthContext) -> Result<()> {
        let old = Payment::get(self, id)?;
        if old.status == Status::Pending {
            let next = PaymentWrite {
                status: Status::Cancelled.into(),
                ..Default::default()
            }
            .update(self, id)?;
            next.audit(self, "payments.cancel", auth, Some(&old))?;
        }
        Ok(())
    }
}
