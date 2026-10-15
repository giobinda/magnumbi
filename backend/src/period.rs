//! Turn a period spec (last 30 days, month to date, from/to; compared with the previous period
//! or year) into concrete UTC windows.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use serde_json::{Value, json};

use crate::schema::{Compare, CompareNamed, PeriodRange, Preset};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPeriod {
    /// current window [start, end), UTC
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// previous window = current shifted back by this many days
    pub offset_days: Option<i64>,
}

fn fmt(d: DateTime<Utc>, ftype: &str) -> String {
    match ftype {
        "date" => d.date_naive().to_string(),
        "datetime" => d.naive_utc().format("%Y-%m-%dT%H:%M:%S%.f").to_string(),
        _ => d.format("%Y-%m-%dT%H:%M:%S%.f+00:00").to_string(),
    }
}

impl ResolvedPeriod {
    /// The windows as inclusive labels, which is how people read them.
    pub fn as_json(&self, ftype: &str) -> Value {
        let last = Duration::microseconds(1);
        let previous = self.offset_days.map(|o| {
            let sh = Duration::days(o);
            json!([fmt(self.start - sh, ftype), fmt(self.end - sh - last, ftype)])
        });
        json!({
            "current": [fmt(self.start, ftype), fmt(self.end - last, ftype)],
            "previous": previous,
            "offset_days": self.offset_days,
        })
    }
}

/// Now, in 5-minute steps: identical SQL for 5 minutes, so database result caches can hit.
pub fn now_rounded() -> DateTime<Utc> {
    let n = Utc::now();
    n.with_minute(n.minute() - n.minute() % 5)
        .and_then(|n| n.with_second(0))
        .and_then(|n| n.with_nanosecond(0))
        .unwrap_or(n)
}

fn midnight(d: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight exists"))
}

/// Windows are whole days and include today. For timestamp fields the current window is cut at
/// `now` and the previous one at the same time of day, so a partly elapsed day is compared with
/// an equally elapsed one.
pub fn resolve_period(
    current: &PeriodRange,
    compare: Option<&Compare>,
    ftype: &str,
    now: DateTime<Utc>,
) -> Result<ResolvedPeriod, String> {
    current.validate()?;
    let today = now.date_naive();
    let tomorrow = today + Duration::days(1);
    let (start, end) = if let Some(n) = current.last_n_days {
        (today - Duration::days(n - 1), tomorrow)
    } else if let Some(p) = current.preset {
        let start = match p {
            Preset::MonthToDate => today.with_day(1),
            Preset::QuarterToDate => NaiveDate::from_ymd_opt(today.year(), 3 * ((today.month() - 1) / 3) + 1, 1),
            Preset::YearToDate => NaiveDate::from_ymd_opt(today.year(), 1, 1),
        }
        .expect("first of month exists");
        (start, tomorrow)
    } else {
        let from = current.from.expect("validated");
        (from, current.to.unwrap_or(today) + Duration::days(1))
    };

    let length = (end - start).num_days();
    let offset = match compare {
        None => None,
        Some(Compare::Named(CompareNamed::PreviousPeriod)) => Some(length),
        Some(Compare::Named(CompareNamed::PreviousYear)) => Some(364), // 52 weeks: Mondays compare with Mondays
        Some(Compare::From(c)) => {
            let offset = (start - c.from).num_days();
            if offset < length {
                return Err("the comparison window must start at least one full period before the current one".into());
            }
            Some(offset)
        }
    };

    let (start_dt, mut end_dt) = (midnight(start), midnight(end));
    // cut the current window at now (local datetimes are approximated by UTC now)
    if (ftype == "time" || ftype == "datetime") && end_dt > now {
        end_dt = now;
    }
    Ok(ResolvedPeriod { start: start_dt, end: end_dt, offset_days: offset })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::CompareFrom;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 15, 7, 30, 0).unwrap()
    }

    fn last(n: i64) -> PeriodRange {
        PeriodRange { last_n_days: Some(n), preset: None, from: None, to: None }
    }

    #[test]
    fn last_n_days_with_previous_period() {
        let p = resolve_period(&last(7), Some(&Compare::Named(CompareNamed::PreviousPeriod)), "date", now()).unwrap();
        assert_eq!(p.start, Utc.with_ymd_and_hms(2026, 10, 9, 0, 0, 0).unwrap());
        assert_eq!(p.end, Utc.with_ymd_and_hms(2026, 10, 16, 0, 0, 0).unwrap());
        assert_eq!(p.offset_days, Some(7));
        let j = p.as_json("date");
        assert_eq!(j["current"], json!(["2026-10-09", "2026-10-15"]));
        assert_eq!(j["previous"], json!(["2026-10-02", "2026-10-08"]));
    }

    #[test]
    fn timestamps_are_cut_at_now() {
        let p = resolve_period(&last(1), Some(&Compare::Named(CompareNamed::PreviousYear)), "time", now()).unwrap();
        assert_eq!(p.end, now());
        assert_eq!(p.offset_days, Some(364));
    }

    #[test]
    fn presets() {
        let q = PeriodRange { last_n_days: None, preset: Some(Preset::QuarterToDate), from: None, to: None };
        let p = resolve_period(&q, None, "date", now()).unwrap();
        assert_eq!(p.start, Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap());
        assert_eq!(p.offset_days, None);
    }

    #[test]
    fn custom_comparison_must_not_overlap() {
        let r = PeriodRange {
            last_n_days: None,
            preset: None,
            from: NaiveDate::from_ymd_opt(2026, 10, 1),
            to: NaiveDate::from_ymd_opt(2026, 10, 10),
        };
        let c = Compare::From(CompareFrom { from: NaiveDate::from_ymd_opt(2026, 9, 25).unwrap() });
        assert!(resolve_period(&r, Some(&c), "date", now()).is_err());
        let c = Compare::From(CompareFrom { from: NaiveDate::from_ymd_opt(2025, 10, 1).unwrap() });
        assert_eq!(resolve_period(&r, Some(&c), "date", now()).unwrap().offset_days, Some(365));
    }
}
