use super::*;
use crate::records::{CatalogPrice, CatalogProduct, Record};
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
        let price = CatalogPrice::get(self, id)?;
        let product = CatalogProduct::get(self, price.product_id)?;
        if price.active != 1 || product.active != 1 {
            return Err(state("Price or product is inactive"));
        }
        if price.amount_minor < 1
            || price.currency.len() != 3
            || !price.currency.bytes().all(|b| b.is_ascii_uppercase())
        {
            return Err(invalid("Invalid catalog price"));
        }
        Ok(Price {
            id,
            product_id: product.id,
            product_code: product.code,
            name: product.name,
            amount_minor: price.amount_minor,
            currency: price.currency,
            billing_interval: price.billing_interval.as_str().into(),
        })
    }
    pub fn b2c_catalog(&self) -> Result<Value> {
        let mut items = Vec::new();
        for product in CatalogProduct::select(self, "WHERE active=1 ORDER BY id LIMIT 1000", &[])? {
            let prices = CatalogPrice::select(
                self,
                "WHERE product_id=? AND active=1 ORDER BY id LIMIT 1000",
                &[&product.id],
            )?
            .iter()
            .map(|r| r.wire(self))
            .collect::<Result<Vec<_>>>()?;
            items.push(json!({"product":product.wire(self)?,"prices":prices}));
        }
        Ok(json!({"items":items}))
    }
}
