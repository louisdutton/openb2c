use super::*;
use chrono::Months;
/// Calendar arithmetic independent of application workflow composition.
pub(crate) struct BillingPeriod {
    pub anchor: String,
    pub start: String,
    pub end: String,
}
pub(crate) fn billing_period(
    anchor: Option<&str>,
    previous_end: Option<&str>,
    interval: &str,
    cycle: i64,
    now: DateTime<Utc>,
) -> Result<BillingPeriod> {
    let anchor = anchor.map(instant).transpose()?.unwrap_or(now);
    let months = match interval {
        "month" => 1u32,
        "year" => 12u32,
        _ => return Err(invalid("Unsupported billing interval")),
    };
    let count = cycle
        .checked_add(1)
        .and_then(|c| u32::try_from(c).ok())
        .and_then(|c| c.checked_mul(months))
        .ok_or_else(|| invalid("Billing cycle overflow"))?;
    let end = anchor
        .checked_add_months(Months::new(count))
        .ok_or_else(|| invalid("Billing date overflow"))?;
    let start = if cycle == 0 {
        anchor.to_rfc3339()
    } else {
        previous_end
            .ok_or_else(|| invalid("Missing prior billing period"))?
            .into()
    };
    Ok(BillingPeriod {
        anchor: anchor.to_rfc3339(),
        start,
        end: end.to_rfc3339(),
    })
}
