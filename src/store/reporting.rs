use rusqlite::params;

use super::{
    Application, BTreeMap, BTreeSet, DeviceUsage, NaiveDate, Store, Usage, coverage, day_start,
};

pub(crate) type AppTotals = BTreeMap<String, (Application, Usage)>;

#[derive(Default)]
pub(crate) struct PeriodUsage {
    pub apps: AppTotals,
    pub grouped_apps: BTreeMap<String, AppTotals>,
    pub websites: BTreeMap<String, WebsiteTime>,
    pub devices: Vec<DeviceUsage>,
}

#[derive(Default)]
pub(crate) struct WebsiteTime {
    pub chrome: i64,
    pub firefox: i64,
    pub group: Option<String>,
    pub sessions: i64,
}

struct WebsiteSpan {
    domain: String,
    start: i64,
    end: i64,
}

impl Store {
    pub fn save_website_group(&mut self, domain: &str, name: &str) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        if name.is_empty() {
            tx.execute("DELETE FROM website_groups WHERE domain = ?1", [domain])?;
        } else {
            let key = name.to_lowercase();
            let names = Self::folder_names(&tx)?;
            let name = names
                .iter()
                .find(|name| name.to_lowercase() == key)
                .map_or(name, String::as_str);
            tx.execute(
                "INSERT INTO website_groups(domain, name) VALUES (?1, ?2)
                 ON CONFLICT(domain) DO UPDATE SET name = excluded.name",
                params![domain, name],
            )?;
        }
        tx.commit()
    }

    pub(crate) fn report_usage(
        &self,
        start: NaiveDate,
        end: NaiveDate,
        device: Option<&str>,
        include_devices: bool,
    ) -> rusqlite::Result<PeriodUsage> {
        let (Some(start), Some(end)) = (day_start(start), day_start(end)) else {
            return Ok(PeriodUsage::default());
        };
        let sessions = self.range(start, end, device)?;
        let pieces = coverage::covered(&sessions);
        let mut result = PeriodUsage::default();
        if include_devices {
            result.devices = coverage::device_usage(&sessions);
        }
        let browser_sessions: BTreeSet<_> = pieces
            .iter()
            .filter(|part| part.session.app.id.browser_brand().is_some())
            .map(|part| part.session.id)
            .collect();
        let mut spans = BTreeMap::<i64, Vec<WebsiteSpan>>::new();
        let mut groups = BTreeMap::<String, String>::new();
        if !browser_sessions.is_empty() {
            let mut query = self.0.prepare(
                "SELECT focus.session_id, focus.domain, focus.start, focus.end, website_groups.name
                 FROM website_focus AS focus
                 LEFT JOIN website_groups ON website_groups.domain = focus.domain
                 WHERE focus.end > ?1 AND focus.start < ?2
                 ORDER BY focus.session_id, focus.start, focus.id",
            )?;
            let mut rows = query.query(params![start, end])?;
            while let Some(row) = rows.next()? {
                let id = row.get(0)?;
                if !browser_sessions.contains(&id) {
                    continue;
                }
                let session_spans = spans.entry(id).or_default();
                let start: i64 = row.get(2)?;
                let end: i64 = row.get(3)?;
                // Uploaded spans can overlap. The earlier span owns the overlap.
                let start = session_spans
                    .last()
                    .map_or(start, |prior| start.max(prior.end));
                if start < end {
                    let domain: String = row.get(1)?;
                    if let Some(group) = row.get(4)? {
                        groups.entry(domain.clone()).or_insert(group);
                    }
                    session_spans.push(WebsiteSpan { domain, start, end });
                }
            }
        }
        let mut counted = BTreeSet::new();
        for part in pieces {
            let session = part.session;
            let app = &session.app;
            let time = part.end - part.start;
            let starts = session.original_start >= start && counted.insert(session.id);
            add_app_time(&mut result.apps, app, time, starts);
            let mut app_time = time;
            let mut app_start = starts;
            if let Some(spans) = spans.get(&session.id) {
                let first = spans.partition_point(|span| span.end <= part.start);
                let chrome = app.id.name.eq_ignore_ascii_case("chrome");
                for span in spans[first..]
                    .iter()
                    .take_while(|span| span.start < part.end)
                {
                    let time = span.end.min(part.end) - span.start.max(part.start);
                    let site = result
                        .websites
                        .entry(span.domain.clone())
                        .or_insert_with(|| WebsiteTime {
                            group: groups.get(&span.domain).cloned(),
                            ..WebsiteTime::default()
                        });
                    if chrome {
                        site.chrome += time;
                    } else {
                        site.firefox += time;
                    }
                    if site.group.is_some() {
                        app_time -= time;
                        if starts && span.start <= part.start {
                            site.sessions += 1;
                            app_start = false;
                        }
                    }
                }
            }
            if app_time > 0 || app_start {
                add_app_time(
                    result
                        .grouped_apps
                        .entry(crate::report::group_key(app))
                        .or_default(),
                    app,
                    app_time,
                    app_start,
                );
            }
        }
        Ok(result)
    }
}

fn add_app_time(apps: &mut AppTotals, app: &Application, time: i64, starts: bool) {
    let (_, usage) = apps
        .entry(app.id.key.clone())
        .or_insert_with(|| (app.clone(), Usage::default()));
    usage.time += time;
    usage.sessions += i64::from(starts);
}
