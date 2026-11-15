use super::*;
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Price {
    pub id: i64,
    pub product_id: i64,
    pub product_code: String,
    pub name: String,
    pub amount_minor: i64,
    pub currency: String,
    pub billing_interval: String,
}
impl Service {
    pub(super) fn cap_price(&self, id: i64) -> Result<Price> {
        let price = self.raw(self.cap_table("catalog", "price")?, &json!(id))?;
        let product = self.raw(self.cap_table("catalog", "product")?, &price["product_id"])?;
        if price["active"] != 1 || product["active"] != 1 {
            return Err(state("Price or product is inactive"));
        }
        let amount = integer(&price, "amount_minor")?;
        let currency = text(&price, "currency")?;
        if amount < 1 || currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(invalid("Invalid catalog price"));
        }
        Ok(Price {
            id,
            product_id: integer(&product, "id")?,
            product_code: text(&product, "code")?.into(),
            name: text(&product, "name")?.into(),
            amount_minor: amount,
            currency: currency.into(),
            billing_interval: text(&price, "billing_interval")?.into(),
        })
    }
    pub fn b2c_catalog(&self) -> Result<Value> {
        let products = self.cap_table("catalog", "product")?;
        let prices = self.cap_table("catalog", "price")?;
        let items = self.query(
            &format!(
                "SELECT * FROM {} WHERE active=1 ORDER BY id LIMIT 1000",
                quote(products)
            ),
            &[],
        )?;
        let mut result = Vec::new();
        for product in items {
            let offers = self.query(
                &format!(
                    "SELECT * FROM {} WHERE product_id=? AND active=1 ORDER BY id LIMIT 1000",
                    quote(prices)
                ),
                &[sql_value(&product["id"])?],
            )?;
            let product = self.cap_response(products, product)?;
            let offers = offers
                .into_iter()
                .map(|row| self.cap_response(prices, row))
                .collect::<Result<Vec<_>>>()?;
            result.push(json!({"product":product,"prices":offers}));
        }
        Ok(json!({"items":result}))
    }
}
