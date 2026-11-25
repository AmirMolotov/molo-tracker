use std::collections::BTreeMap;

use chrono::{Datelike, Days, Months, NaiveDate};

use crate::store::{Application, Store, Usage};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    #[default]
    Daily,
    Weekly,
    Monthly,
}

impl Period {
    pub fn start(self, date: NaiveDate) -> NaiveDate {
        match self {
            Self::Daily => date,
            Self::Weekly => date - Days::new(date.weekday().num_days_from_monday().into()),
            Self::Monthly => date.with_day(1).unwrap(),
        }
    }

    pub fn adjacent(self, start: NaiveDate, next: bool) -> Option<NaiveDate> {
        let days = match self {
            Self::Daily => Days::new(1),
            Self::Weekly => Days::new(7),
            Self::Monthly => {
                let month = Months::new(1);
                return if next {
                    start.checked_add_months(month)
                } else {
                    start.checked_sub_months(month)
                };
            }
        };
        if next {
            start.checked_add_days(days)
        } else {
            start.checked_sub_days(days)
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
        }
    }
}

pub fn date_range(start: NaiveDate, end: NaiveDate) -> String {
    let last = end.pred_opt().unwrap_or(start);
    if start == last {
        start.format("%b %-d, %Y").to_string()
    } else if start.year() == last.year() {
        format!("{} – {}", start.format("%b %-d"), last.format("%b %-d, %Y"))
    } else {
        format!(
            "{} – {}",
            start.format("%b %-d, %Y"),
            last.format("%b %-d, %Y")
        )
    }
}

#[derive(Default)]
pub struct Report {
    pub groups: Vec<GroupUsage>,
    pub apps: Vec<AppUsage>,
    pub total: Usage,
    pub previous: Usage,
    pub websites: Vec<WebsiteUsage>,
    pub devices: Vec<crate::store::DeviceUsage>,
}

#[derive(Default)]
pub struct WebsiteUsage {
    pub domain: String,
    pub chrome: i64,
    pub firefox: i64,
    pub previous: i64,
}

impl WebsiteUsage {
    pub fn total(&self) -> i64 {
        self.chrome + self.firefox
    }
}

pub struct AppUsage {
    pub app: Application,
    pub current: Usage,
    pub previous: Usage,
}

#[derive(Default)]
pub struct GroupUsage {
    pub key: String,
    pub name: String,
    pub current: Usage,
    pub previous: Usage,
}

impl Usage {
    fn accumulate(&mut self, other: Self) {
        self.time += other.time;
        self.sessions += other.sessions;
    }
}

impl Report {
    pub fn load(
        store: &Store,
        period: Period,
        date: NaiveDate,
        device: Option<&str>,
    ) -> rusqlite::Result<Self> {
        let start = period.start(date);
        let (Some(previous), Some(end)) =
            (period.adjacent(start, false), period.adjacent(start, true))
        else {
            return Ok(Self::default());
        };
        let mut by_app = BTreeMap::new();
        for (app, usage) in store.usage(previous, start, device)? {
            by_app.insert(
                app.id.key.clone(),
                AppUsage {
                    app,
                    current: Usage::default(),
                    previous: usage,
                },
            );
        }
        for (app, usage) in store.usage(start, end, device)? {
            by_app
                .entry(app.id.key.clone())
                .or_insert(AppUsage {
                    app,
                    current: Usage::default(),
                    previous: Usage::default(),
                })
                .current = usage;
        }

        let mut result = Self::default();
        if device.is_none() {
            result.devices = store.devices(start, end)?;
        }
        let mut groups = BTreeMap::new();
        for app in by_app.into_values() {
            let key = group_key(&app.app);
            let group = groups.entry(key.clone()).or_insert_with(|| GroupUsage {
                key,
                name: app.app.group.clone().unwrap_or_else(|| "Ungrouped".into()),
                ..GroupUsage::default()
            });
            group.current.accumulate(app.current);
            group.previous.accumulate(app.previous);
            result.total.accumulate(app.current);
            result.previous.accumulate(app.previous);
            result.apps.push(app);
        }
        result.apps.sort_by(|a, b| {
            b.current
                .time
                .cmp(&a.current.time)
                .then_with(|| b.previous.time.cmp(&a.previous.time))
                .then_with(|| a.app.label().cmp(b.app.label()))
        });
        result.groups = groups.into_values().collect();
        result.groups.sort_by(|a, b| {
            b.current
                .time
                .cmp(&a.current.time)
                .then_with(|| a.name.cmp(&b.name))
        });
        let mut domains = BTreeMap::new();
        for data in store.website_time(previous, start, device)? {
            domains.insert(
                data.domain.clone(),
                WebsiteUsage {
                    domain: data.domain,
                    previous: data.chrome + data.firefox,
                    ..WebsiteUsage::default()
                },
            );
        }
        for data in store.website_time(start, end, device)? {
            let website = domains
                .entry(data.domain.clone())
                .or_insert_with(|| WebsiteUsage {
                    domain: data.domain,
                    ..WebsiteUsage::default()
                });
            website.chrome = data.chrome;
            website.firefox = data.firefox;
        }
        result.websites = domains.into_values().collect();
        result.websites.sort_by(|a, b| {
            b.total()
                .cmp(&a.total())
                .then_with(|| b.previous.cmp(&a.previous))
                .then_with(|| a.domain.cmp(&b.domain))
        });
        Ok(result)
    }
}

pub fn group_key(app: &Application) -> String {
    app.group
        .as_deref()
        .map_or_else(String::new, str::to_lowercase)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::{Local, TimeZone};

    use super::*;
    use crate::platform::AppId;

    #[test]
    fn report_groups_time_and_compares_sessions_across_midnight() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let midnight = Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis();
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let shooter = AppId {
            key: "shooter".into(),
            name: "Shooter".into(),
        };
        let puzzle = AppId {
            key: "puzzle".into(),
            name: "Puzzle".into(),
        };
        store
            .record(&shooter, midnight - 2000, midnight + 4000, None, None, None)
            .unwrap();
        store
            .record(&puzzle, midnight + 8000, midnight + 13000, None, None, None)
            .unwrap();
        store
            .save_groups(std::slice::from_ref(&shooter), "Games")
            .unwrap();
        store
            .save_groups(std::slice::from_ref(&puzzle), "games")
            .unwrap();

        let report = Report::load(&store, Period::Daily, day, None).unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!((report.total.time, report.total.sessions), (9000, 1));
        assert_eq!((report.previous.time, report.previous.sessions), (2000, 1));
        assert_eq!(report.groups[0].current.time, 9000);
        assert_eq!(Period::Weekly.start(day.succ_opt().unwrap()), day);
        assert_eq!(
            Period::Monthly.adjacent(Period::Monthly.start(day), true),
            NaiveDate::from_ymd_opt(2026, 12, 1)
        );
    }

    #[test]
    fn device_report_applies_coverage_and_comparison_to_that_device() {
        use crate::sync::{Delta, Span};

        let day = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let midnight = Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis();
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let chrome = AppId {
            key: "chrome.exe".into(),
            name: "chrome".into(),
        };
        store
            .record(
                &chrome,
                midnight,
                midnight + 5000,
                None,
                None,
                Some("local.example"),
            )
            .unwrap();
        store.save_groups(&[chrome], "Work").unwrap();
        let remote = "remote-pc";
        let session = |id, start, end| Delta {
            device: remote.into(),
            id,
            app: "firefox".into(),
            start,
            end,
            pages: Vec::new(),
            domains: vec![Span {
                value: "remote.example".into(),
                start,
                end,
            }],
        };
        store
            .apply_sync(
                &[],
                1,
                &[
                    session(1, midnight - 3000, midnight),
                    session(2, midnight + 1000, midnight + 4000),
                ],
                &BTreeMap::from([(remote.into(), "Laptop".into())]),
                None,
            )
            .unwrap();

        let devices = store.report_devices().unwrap();
        assert!(devices[0].local);
        assert_eq!(
            (devices[1].id.as_str(), devices[1].name.as_str()),
            (remote, "Laptop")
        );
        let combined = Report::load(&store, Period::Daily, day, None).unwrap();
        assert_eq!(combined.total.time, 5000);
        assert_eq!(
            combined
                .devices
                .iter()
                .map(|device| device.time)
                .sum::<i64>(),
            8000
        );

        let local = Report::load(&store, Period::Daily, day, Some(&devices[0].id)).unwrap();
        assert_eq!(
            (local.total.time, local.total.sessions, local.previous.time),
            (5000, 1, 0)
        );
        assert_eq!(
            (local.groups[0].name.as_str(), local.groups[0].current.time),
            ("Work", 5000)
        );
        assert_eq!(local.websites.len(), 1);
        assert_eq!(
            (local.websites[0].domain.as_str(), local.websites[0].chrome),
            ("local.example", 5000)
        );

        let laptop = Report::load(&store, Period::Daily, day, Some(remote)).unwrap();
        assert_eq!(
            (
                laptop.total.time,
                laptop.total.sessions,
                laptop.previous.time
            ),
            (3000, 1, 3000)
        );
        assert_eq!(laptop.apps.len(), 1);
        assert_eq!(laptop.apps[0].app.label(), "firefox");
        assert_eq!(laptop.websites.len(), 1);
        assert_eq!(
            (laptop.websites[0].firefox, laptop.websites[0].previous),
            (3000, 3000)
        );
        let later =
            Report::load(&store, Period::Daily, day.succ_opt().unwrap(), Some(remote)).unwrap();
        assert_eq!((later.total.time, later.previous.time), (0, 3000));
    }

    #[test]
    fn website_report_combines_browsers_and_clips_previous_period() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let midnight = Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis();
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let chrome = AppId {
            key: "chrome.exe".into(),
            name: "chrome".into(),
        };
        let firefox = AppId {
            key: "firefox.exe".into(),
            name: "firefox".into(),
        };
        let id = store
            .record(
                &chrome,
                midnight - 2000,
                midnight + 1000,
                None,
                None,
                Some("example.com"),
            )
            .unwrap();
        store
            .record(
                &chrome,
                midnight + 1000,
                midnight + 2000,
                Some(id),
                None,
                Some("example.com"),
            )
            .unwrap();
        store
            .record(
                &chrome,
                midnight + 2000,
                midnight + 3000,
                Some(id),
                None,
                None,
            )
            .unwrap();
        store
            .record(
                &firefox,
                midnight + 3000,
                midnight + 6000,
                None,
                None,
                Some("example.com"),
            )
            .unwrap();

        let report = Report::load(&store, Period::Daily, day, None).unwrap();
        assert_eq!(report.websites.len(), 1);
        let site = &report.websites[0];
        assert_eq!(site.domain, "example.com");
        assert_eq!(
            (site.chrome, site.firefox, site.total(), site.previous),
            (2000, 3000, 5000, 2000)
        );
        assert_eq!(report.total.time, 6000);
    }
}
