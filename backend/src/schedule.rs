//! Scheduled reports: when they run, and who may receive them. The rest (running them, sending
//! them) is in api/schedules.rs.

use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// When a report runs: every day, a day of the week (1 Monday .. 7 Sunday) or a day of the month
/// (1 to 28: every month has it), at a local time in a time zone.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// daily | weekly | monthly
    pub every: String,
    #[serde(default)]
    pub day: Option<u8>,
    /// HH:MM
    pub at: String,
    /// e.g. Europe/Rome
    pub timezone: String,
}

impl When {
    pub fn check(&self) -> Result<(), String> {
        match (self.every.as_str(), self.day) {
            ("daily", _) => {}
            ("weekly", Some(1..=7)) => {}
            ("weekly", _) => return Err("a weekly report needs its day of the week (1 Monday to 7 Sunday)".into()),
            ("monthly", Some(1..=28)) => {}
            ("monthly", _) => return Err("a monthly report runs on a day from 1 to 28".into()),
            _ => return Err("reports run daily, weekly or monthly".into()),
        }
        self.time().ok_or_else(|| format!("{:?}: a time as HH:MM", self.at))?;
        self.timezone.parse::<Tz>().map_err(|_| format!("{:?} isn't a time zone", self.timezone))?;
        Ok(())
    }

    fn time(&self) -> Option<NaiveTime> {
        if self.at.len() != 5 {
            return None;
        }
        NaiveTime::parse_from_str(&self.at, "%H:%M").ok()
    }

    fn runs_on(&self, d: NaiveDate) -> bool {
        match (self.every.as_str(), self.day) {
            ("weekly", Some(w)) => d.weekday().number_from_monday() == u32::from(w),
            ("monthly", Some(m)) => d.day() == u32::from(m),
            _ => true,
        }
    }

    /// The first run strictly after `t`. A local time that doesn't exist that day (clocks going
    /// forward) runs an hour later; one that happens twice runs the first time.
    pub fn next_after(&self, t: DateTime<Utc>) -> DateTime<Utc> {
        let tz: Tz = self.timezone.parse().unwrap_or(Tz::UTC);
        let time = self.time().unwrap_or(NaiveTime::MIN);
        let today = t.with_timezone(&tz).date_naive();
        for offset in 0..=62 {
            let Some(date) = today.checked_add_days(Days::new(offset)) else { break };
            if !self.runs_on(date) {
                continue;
            }
            let local = date.and_time(time);
            let instant = tz
                .from_local_datetime(&local)
                .earliest()
                .or_else(|| tz.from_local_datetime(&(local + TimeDelta::hours(1))).earliest());
            if let Some(i) = instant.map(|i| i.with_timezone(&Utc))
                && i > t
            {
                return i;
            }
        }
        t + TimeDelta::days(1) // not reached for checked schedules
    }
}

/// What a scheduled report sends, and to whom.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleConfig {
    pub name: String,
    pub when: When,
    /// csv (a zip of the charts' data) | signed (a guest link, with a password) | public (a public link)
    pub format: String,
    #[serde(default)]
    pub recipients: Vec<String>,
    /// the tenant's webhooks, by name
    #[serde(default)]
    pub webhooks: Vec<String>,
    /// the dashboard's filters and period, as a viewer would ask for a tile
    #[serde(default)]
    pub request: serde_json::Value,
    /// signed: the guests' role
    #[serde(default)]
    pub guest_role: Option<String>,
    /// public: the public link (its id) to send
    #[serde(default)]
    pub share_link: Option<String>,
}

/// Whether a report may be sent to `email`: one of the tenant's members, someone of a domain its
/// people join by, or of a domain its Org admins allow (exactly that domain).
pub fn may_receive(email: &str, members: &[String], member_domains: &[String], allowed_domains: &[String]) -> bool {
    let email = email.trim().to_lowercase();
    let Some((local, domain)) = email.split_once('@') else { return false };
    if local.is_empty() || domain.is_empty() || domain.contains('@') || email.contains(char::is_whitespace) {
        return false;
    }
    let same = |d: &String| d.trim().trim_start_matches('@').eq_ignore_ascii_case(domain);
    members.iter().any(|m| m.eq_ignore_ascii_case(&email))
        || member_domains.iter().any(same)
        || allowed_domains.iter().any(same)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn when(every: &str, day: Option<u8>, time: &str, tz: &str) -> When {
        When { every: every.into(), day, at: time.into(), timezone: tz.into() }
    }

    #[test]
    fn daily_reports_run_at_local_time_through_daylight_saving() {
        let w = when("daily", None, "08:00", "Europe/Rome");
        assert_eq!(w.next_after(at("2026-10-19T05:00:00Z")), at("2026-10-19T06:00:00Z"), "08:00 summer time");
        assert_eq!(w.next_after(at("2026-10-19T06:00:00Z")), at("2026-10-20T06:00:00Z"), "strictly after");
        assert_eq!(w.next_after(at("2026-10-24T07:00:00Z")), at("2026-10-25T07:00:00Z"), "08:00 winter time");
        // 02:30 doesn't exist on the night clocks go forward: the hour after
        let night = when("daily", None, "02:30", "Europe/Rome");
        assert_eq!(night.next_after(at("2027-03-27T12:00:00Z")), at("2027-03-28T01:30:00Z"));
    }

    #[test]
    fn weekly_and_monthly_reports() {
        // Monday 09:30 in New York, asked on a Wednesday
        let w = when("weekly", Some(1), "09:30", "America/New_York");
        assert_eq!(w.next_after(at("2026-10-21T12:00:00Z")), at("2026-10-26T13:30:00Z"));
        let m = when("monthly", Some(28), "07:00", "UTC");
        assert_eq!(m.next_after(at("2026-10-28T07:00:00Z")), at("2026-11-28T07:00:00Z"));
        assert_eq!(m.next_after(at("2026-12-29T00:00:00Z")), Utc.with_ymd_and_hms(2027, 1, 28, 7, 0, 0).unwrap());
    }

    #[test]
    fn schedules_are_checked() {
        assert_eq!(when("daily", None, "08:00", "Europe/Rome").check(), Ok(()));
        for (w, why) in [
            (when("hourly", None, "08:00", "UTC"), "daily, weekly or monthly"),
            (when("weekly", None, "08:00", "UTC"), "day of the week"),
            (when("weekly", Some(8), "08:00", "UTC"), "day of the week"),
            (when("monthly", Some(31), "08:00", "UTC"), "1 to 28"),
            (when("daily", None, "25:00", "UTC"), "HH:MM"),
            (when("daily", None, "8", "UTC"), "HH:MM"),
            (when("daily", None, "08:00", "Mars/Olympus"), "time zone"),
        ] {
            let e = w.check().unwrap_err();
            assert!(e.contains(why), "{w:?}: {e}");
        }
    }

    #[test]
    fn reports_go_to_members_and_allowed_domains_only() {
        let members = vec!["ana@acme.example".to_string()];
        let member_domains = vec!["acme.example".to_string()];
        let allowed = vec!["partner.example".to_string()];
        let ok = |e: &str| may_receive(e, &members, &member_domains, &allowed);
        assert!(ok("ana@acme.example") && ok("Ana@ACME.example"));
        assert!(ok("bob@acme.example"), "people who join by domain");
        assert!(ok("eve@partner.example"), "a domain the Org admin allows");
        assert!(!ok("eve@evil.example"));
        assert!(!ok("eve@sub.partner.example"), "exactly the domain");
        assert!(!ok("eve@partner.example.evil.example"));
        assert!(!ok("not an address"));
        assert!(!ok("a@b@partner.example"));
    }
}
