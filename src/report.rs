use std::collections::BTreeMap;

use chrono::{Datelike, Days, Months, NaiveDate};

use crate::store::{AppTotals, Application, Store, Usage};

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
    pub group: Option<String>,
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
    pub apps: Vec<AppUsage>,
}

impl GroupUsage {
    fn new(name: Option<&str>) -> Self {
        Self {
            key: name.map_or_else(String::new, str::to_lowercase),
            name: name.unwrap_or("Ungrouped").into(),
            ..Self::default()
        }
    }
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
        let prior = store.report_usage(previous, start, device, false)?;
        let current = store.report_usage(start, end, device, device.is_none())?;
        let mut result = Self {
            apps: compare_apps(current.apps, prior.apps),
            devices: current.devices,
            ..Self::default()
        };
        for app in &result.apps {
            result.total.accumulate(app.current);
            result.previous.accumulate(app.previous);
        }

        let mut groups = BTreeMap::new();
        let mut prior_apps = prior.grouped_apps;
        for (key, apps) in current.grouped_apps {
            let previous = prior_apps.remove(&key).unwrap_or_default();
            insert_app_group(&mut groups, compare_apps(apps, previous));
        }
        for apps in prior_apps.into_values() {
            insert_app_group(&mut groups, compare_apps(AppTotals::new(), apps));
        }

        let mut domains = BTreeMap::new();
        for (period, is_current) in [(prior.websites, false), (current.websites, true)] {
            for (domain, data) in period {
                let time = data.chrome + data.firefox;
                if let Some(name) = &data.group {
                    let group = groups
                        .entry(name.to_lowercase())
                        .or_insert_with(|| GroupUsage::new(Some(name)));
                    let usage = if is_current {
                        &mut group.current
                    } else {
                        &mut group.previous
                    };
                    usage.accumulate(Usage {
                        time,
                        sessions: data.sessions,
                    });
                }
                let website = domains
                    .entry(domain.clone())
                    .or_insert_with(|| WebsiteUsage {
                        domain,
                        group: data.group,
                        ..WebsiteUsage::default()
                    });
                if is_current {
                    website.chrome = data.chrome;
                    website.firefox = data.firefox;
                } else {
                    website.previous = time;
                }
            }
        }
        result.groups = groups.into_values().collect();
        result.groups.sort_by(|a, b| {
            b.current
                .time
                .cmp(&a.current.time)
                .then_with(|| a.name.cmp(&b.name))
        });
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

fn compare_apps(current: AppTotals, previous: AppTotals) -> Vec<AppUsage> {
    let mut by_app = BTreeMap::new();
    for (key, (app, usage)) in previous {
        by_app.insert(
            key,
            AppUsage {
                app,
                current: Usage::default(),
                previous: usage,
            },
        );
    }
    for (key, (app, usage)) in current {
        by_app
            .entry(key)
            .or_insert(AppUsage {
                app,
                current: Usage::default(),
                previous: Usage::default(),
            })
            .current = usage;
    }
    let mut apps: Vec<_> = by_app.into_values().collect();
    apps.sort_by(|a, b| {
        b.current
            .time
            .cmp(&a.current.time)
            .then_with(|| b.previous.time.cmp(&a.previous.time))
            .then_with(|| a.app.label().cmp(b.app.label()))
    });
    apps
}

fn insert_app_group(groups: &mut BTreeMap<String, GroupUsage>, apps: Vec<AppUsage>) {
    let mut group = GroupUsage::new(apps.first().and_then(|app| app.app.group.as_deref()));
    for app in &apps {
        group.current.accumulate(app.current);
        group.previous.accumulate(app.previous);
    }
    group.apps = apps;
    groups.insert(group.key.clone(), group);
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
    fn website_and_app_time_share_groups_without_counting_browser_time_twice() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let noon = Local
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis();
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let app = |name: &str| AppId {
            key: name.into(),
            name: name.into(),
        };
        let browser = app("chrome");
        let editor = app("Editor");
        let session = store
            .record(
                &browser,
                noon,
                noon + 2000,
                None,
                None,
                Some("work.example"),
            )
            .unwrap();
        store
            .record(
                &browser,
                noon + 2000,
                noon + 5000,
                Some(session),
                None,
                None,
            )
            .unwrap();
        store
            .record(&editor, noon + 5000, noon + 9000, None, None, None)
            .unwrap();
        store.save_groups(&[editor], "Work").unwrap();
        store.save_groups(&[browser], "Browsing").unwrap();
        store.save_website_group("work.example", "work").unwrap();

        let report = Report::load(&store, Period::Daily, day, None).unwrap();
        assert_eq!((report.total.time, report.total.sessions), (9000, 2));
        assert_eq!(
            (
                report.groups[0].name.as_str(),
                report.groups[0].current.time,
                report.groups[0].current.sessions
            ),
            ("Work", 6000, 2)
        );
        assert_eq!(report.groups[0].apps[0].app.label(), "Editor");
        assert_eq!(
            (
                report.groups[1].name.as_str(),
                report.groups[1].current.time,
                report.groups[1].current.sessions
            ),
            ("Browsing", 3000, 0)
        );
        assert_eq!(
            report
                .apps
                .iter()
                .find(|app| app.app.label() == "chrome")
                .unwrap()
                .current
                .time,
            5000
        );
        assert_eq!(report.websites[0].group.as_deref(), Some("Work"));

        assert!(store.change_folder("work", Some("Projects")).unwrap());
        let report = Report::load(&store, Period::Daily, day, None).unwrap();
        assert_eq!(report.websites[0].group.as_deref(), Some("Projects"));
        assert_eq!(report.groups[0].name, "Projects");
        assert!(store.change_folder("projects", None).unwrap());
        let report = Report::load(&store, Period::Daily, day, None).unwrap();
        assert!(report.websites[0].group.is_none());
        assert_eq!(
            (
                report.groups[0].name.as_str(),
                report.groups[0].current.time
            ),
            ("Browsing", 5000)
        );
    }

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
