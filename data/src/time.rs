use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
pub struct Posix(u64);

impl Posix {
    pub fn now() -> Self {
        let nanos_since_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("valid unix timestamp")
            .as_nanos() as u64;

        Self(nanos_since_epoch)
    }

    pub fn from_seconds(seconds: u64) -> Self {
        Self(seconds * 1_000_000_000)
    }

    pub fn as_nanos(&self) -> u64 {
        self.0
    }

    pub fn datetime(&self) -> Option<DateTime<Utc>> {
        let seconds = (self.0 / 1_000_000_000) as i64;
        let nanos = (self.0 % 1_000_000_000) as u32;

        DateTime::from_timestamp(seconds, nanos)
    }
}

/// Whole days from `date` back to `today`; `None` for a date in the
/// future. The one bucket both relative labels below are built on.
fn days_before(date: NaiveDate, today: NaiveDate) -> Option<u64> {
    (today - date).num_days().try_into().ok()
}

/// Sidebar relative-activity label (QML `formatLastActivity` parity):
/// the time for today, "Yesterday", a short date otherwise.
pub fn relative_day_label(when: DateTime<Utc>) -> String {
    relative_day_label_at(when, Local::now())
}

fn relative_day_label_at(when: DateTime<Utc>, now: DateTime<Local>) -> String {
    let when = when.with_timezone(&now.timezone());

    match days_before(when.date_naive(), now.date_naive()) {
        Some(0) => when.format("%H:%M").to_string(),
        Some(1) => "Yesterday".to_string(),
        _ if when.year() == now.year() => {
            when.format("%e %b").to_string().trim_start().to_string()
        }
        _ => when.format("%x").to_string(),
    }
}

/// Day-separator chip label (QML `MessageListModel::dayLabel` parity):
/// "Today", "Yesterday", then the weekday for the rest of the past week.
/// `None` for anything older, which the caller renders in its own
/// configured date format.
pub fn day_chip_label(date: NaiveDate) -> Option<String> {
    day_chip_label_at(date, Local::now().date_naive())
}

fn day_chip_label_at(date: NaiveDate, today: NaiveDate) -> Option<String> {
    match days_before(date, today)? {
        0 => Some("Today".to_string()),
        1 => Some("Yesterday".to_string()),
        2..=6 => Some(date.format("%A").to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn relative_day_label_matches_qml_buckets() {
        let now = Local.with_ymd_and_hms(2026, 7, 30, 15, 0, 0).unwrap();
        let at = |y, m, d, h| {
            Local
                .with_ymd_and_hms(y, m, d, h, 3, 0)
                .unwrap()
                .with_timezone(&Utc)
        };

        assert_eq!(relative_day_label_at(at(2026, 7, 30, 14), now), "14:03");
        assert_eq!(
            relative_day_label_at(at(2026, 7, 29, 23), now),
            "Yesterday"
        );
        assert_eq!(relative_day_label_at(at(2026, 7, 1, 9), now), "1 Jul");
        assert_eq!(relative_day_label_at(at(2025, 12, 31, 9), now), "12/31/25");
    }

    #[test]
    fn day_chip_label_matches_qml_buckets() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 30).unwrap();
        let on = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();

        assert_eq!(
            day_chip_label_at(on(7, 30), today).as_deref(),
            Some("Today")
        );
        assert_eq!(
            day_chip_label_at(on(7, 29), today).as_deref(),
            Some("Yesterday")
        );
        assert_eq!(
            day_chip_label_at(on(7, 25), today).as_deref(),
            Some("Saturday")
        );
        assert_eq!(day_chip_label_at(on(7, 23), today), None);
        assert_eq!(day_chip_label_at(on(8, 1), today), None);
    }
}
