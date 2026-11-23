use chrono::{Days, Local, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

use crate::platform::AppId;

#[derive(Clone, Debug)]
pub struct Application {
    pub id: AppId,
    pub alias: Option<String>,
    pub group: Option<String>,
}

#[derive(Clone, Copy, Default)]
pub struct Usage {
    pub time: i64,
    pub sessions: i64,
}

impl Application {
    pub fn label(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.id.name)
    }
}

pub struct Session {
    pub id: i64,
    pub app: Application,
    pub start: i64,
    pub end: i64,
    pub full_duration: i64,
}

impl Session {
    pub fn meets_cutoff(&self, seconds: u32) -> bool {
        self.full_duration >= i64::from(seconds) * 1000
    }
}

pub struct BrowserPage {
    pub title: String,
    pub start: i64,
    pub end: i64,
}

pub struct WebsiteTime {
    pub domain: String,
    pub chrome: i64,
    pub firefox: i64,
}

pub struct Store(Connection);

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        // Keep the original chrome_pages table name so existing databases retain browser details.
        db.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS apps (
                 path TEXT PRIMARY KEY, name TEXT NOT NULL, alias TEXT
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 id INTEGER PRIMARY KEY, app TEXT NOT NULL REFERENCES apps(path),
                 start INTEGER NOT NULL, end INTEGER NOT NULL CHECK(end > start)
             );
             CREATE INDEX IF NOT EXISTS sessions_end ON sessions(end);
             CREATE INDEX IF NOT EXISTS sessions_start ON sessions(start);
             CREATE TABLE IF NOT EXISTS app_groups (
                 app TEXT PRIMARY KEY REFERENCES apps(path),
                 name TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS group_folders (
                 name TEXT PRIMARY KEY
             );
             CREATE TABLE IF NOT EXISTS chrome_pages (
                 id INTEGER PRIMARY KEY,
                 session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 title TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL CHECK(end > start)
             );
             CREATE INDEX IF NOT EXISTS chrome_pages_session_end ON chrome_pages(session_id, end);
             CREATE TABLE IF NOT EXISTS website_focus (
                 id INTEGER PRIMARY KEY,
                 session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 domain TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL CHECK(end > start)
             );
             CREATE INDEX IF NOT EXISTS website_focus_end ON website_focus(end);
             CREATE INDEX IF NOT EXISTS website_focus_session_end ON website_focus(session_id, end);
             CREATE TABLE IF NOT EXISTS settings (
                 id INTEGER PRIMARY KEY CHECK(id = 1),
                 cutoff INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO settings(id, cutoff) VALUES (1, 5);",
        )?;
        Ok(Self(db))
    }

    pub fn cutoff(&self) -> rusqlite::Result<u32> {
        self.0
            .query_row("SELECT cutoff FROM settings WHERE id = 1", [], |row| {
                row.get(0)
            })
    }

    pub fn save_cutoff(&self, seconds: u32) -> rusqlite::Result<()> {
        self.0
            .execute("UPDATE settings SET cutoff = ?1 WHERE id = 1", [seconds])?;
        Ok(())
    }

    fn register(db: &Connection, app: &AppId) -> rusqlite::Result<()> {
        db.execute(
            "INSERT OR IGNORE INTO apps(path, name) VALUES(?1, ?2)",
            params![app.key, app.name],
        )?;
        Ok(())
    }

    pub fn save_alias(&mut self, app: &AppId, value: &str) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        Self::register(&tx, app)?;
        let alias = Some(value.trim()).filter(|alias| !alias.is_empty());
        tx.execute(
            "UPDATE apps SET alias = ?2 WHERE path = ?1",
            params![app.key, alias],
        )?;
        tx.commit()
    }

    pub fn apps(&self) -> rusqlite::Result<Vec<Application>> {
        let mut query = self.0.prepare(
            "SELECT apps.path, apps.name, apps.alias, app_groups.name
             FROM apps LEFT JOIN app_groups ON app_groups.app = apps.path",
        )?;
        query.query_map([], read_app)?.collect()
    }

    pub fn group_folders(&self) -> rusqlite::Result<Vec<String>> {
        let mut query = self
            .0
            .prepare("SELECT name FROM group_folders UNION SELECT name FROM app_groups")?;
        query.query_map([], |row| row.get(0))?.collect()
    }

    pub fn save_groups(&mut self, apps: &[AppId], name: &str) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        if !name.is_empty() {
            tx.execute(
                "INSERT OR IGNORE INTO group_folders(name) VALUES (?1)",
                [name],
            )?;
        }
        for app in apps {
            Self::register(&tx, app)?;
            if name.is_empty() {
                tx.execute("DELETE FROM app_groups WHERE app = ?1", [&app.key])?;
            } else {
                tx.execute(
                    "INSERT INTO app_groups(app, name) VALUES(?1, ?2)
                     ON CONFLICT(app) DO UPDATE SET name = excluded.name",
                    params![app.key, name],
                )?;
            }
        }
        tx.commit()
    }

    pub fn usage(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> rusqlite::Result<Vec<(Application, Usage)>> {
        let (Some(start), Some(end)) = (day_start(start), day_start(end)) else {
            return Ok(Vec::new());
        };
        let mut query = self.0.prepare(
            "SELECT apps.path, apps.name, apps.alias, app_groups.name,
                    SUM(MIN(sessions.end, ?2) - MAX(sessions.start, ?1)), SUM(sessions.start >= ?1)
             FROM sessions JOIN apps ON apps.path = sessions.app
             LEFT JOIN app_groups ON app_groups.app = apps.path
             WHERE sessions.end > ?1 AND sessions.start < ?2
             GROUP BY apps.path",
        )?;
        query
            .query_map(params![start, end], |row| {
                Ok((
                    read_app(row)?,
                    Usage {
                        time: row.get(4)?,
                        sessions: row.get(5)?,
                    },
                ))
            })?
            .collect()
    }

    pub fn record(
        &mut self,
        app: &AppId,
        start: i64,
        end: i64,
        continuation: Option<i64>,
        page_title: Option<&str>,
        domain: Option<&str>,
    ) -> rusqlite::Result<i64> {
        let tx = self.0.transaction()?;
        let mut continued = None;
        if let Some(id) = continuation
            && tx.execute(
                "UPDATE sessions SET end = ?1 WHERE id = ?2 AND end = ?3 AND app = ?4",
                params![end, id, start, app.key],
            )? == 1
        {
            continued = Some(id);
        }
        let id = match continued {
            Some(id) => id,
            None => {
                Self::register(&tx, app)?;
                tx.query_row(
                    "INSERT INTO sessions(app, start, end) VALUES(?1, ?2, ?3) RETURNING id",
                    params![app.key, start, end],
                    |row| row.get(0),
                )?
            }
        };
        if let Some(title) = page_title
            && tx.execute(
                "UPDATE chrome_pages SET end = ?1 WHERE session_id = ?2 AND end = ?3 AND title = ?4",
                params![end, id, start, title],
            )? == 0
        {
            tx.execute(
                "INSERT INTO chrome_pages(session_id, title, start, end) VALUES(?1, ?2, ?3, ?4)",
                params![id, title, start, end],
            )?;
        }
        if let Some(domain) = domain
            && tx.execute(
                "UPDATE website_focus SET end = ?1 WHERE session_id = ?2 AND end = ?3 AND domain = ?4",
                params![end, id, start, domain],
            )? == 0
        {
            tx.execute(
                "INSERT INTO website_focus(session_id, domain, start, end) VALUES(?1, ?2, ?3, ?4)",
                params![id, domain, start, end],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn website_time(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> rusqlite::Result<Vec<WebsiteTime>> {
        let (Some(start), Some(end)) = (day_start(start), day_start(end)) else {
            return Ok(Vec::new());
        };
        let mut query = self.0.prepare(
            "SELECT website_focus.domain,
                SUM(CASE WHEN lower(apps.name) = 'chrome' THEN MIN(website_focus.end, ?2) - MAX(website_focus.start, ?1) ELSE 0 END),
                SUM(CASE WHEN lower(apps.name) = 'firefox' THEN MIN(website_focus.end, ?2) - MAX(website_focus.start, ?1) ELSE 0 END)
             FROM website_focus JOIN sessions ON sessions.id = website_focus.session_id
             JOIN apps ON apps.path = sessions.app
             WHERE website_focus.end > ?1 AND website_focus.start < ?2
             GROUP BY website_focus.domain",
        )?;
        query
            .query_map(params![start, end], |row| {
                Ok(WebsiteTime {
                    domain: row.get(0)?,
                    chrome: row.get(1)?,
                    firefox: row.get(2)?,
                })
            })?
            .collect()
    }

    pub fn pages(&self, session: &Session) -> rusqlite::Result<Vec<BrowserPage>> {
        let mut query = self.0.prepare(
            "SELECT title, MAX(start, ?2), MIN(end, ?3) FROM chrome_pages
             WHERE session_id = ?1 AND end > ?2 AND start < ?3 ORDER BY start",
        )?;
        query
            .query_map(params![session.id, session.start, session.end], |row| {
                Ok(BrowserPage {
                    title: row.get(0)?,
                    start: row.get(1)?,
                    end: row.get(2)?,
                })
            })?
            .collect()
    }

    pub fn day_total(&self, date: NaiveDate) -> rusqlite::Result<i64> {
        let Some((start, end)) = day_bounds(date) else {
            return Ok(0);
        };
        self.0.query_row(
            "SELECT COALESCE(SUM(MIN(end, ?2) - MAX(start, ?1)), 0) FROM sessions
             WHERE end > ?1 AND start < ?2",
            params![start, end],
            |row| row.get(0),
        )
    }

    pub fn day(&self, date: NaiveDate) -> rusqlite::Result<Vec<Session>> {
        let Some((start, end)) = day_bounds(date) else {
            return Ok(Vec::new());
        };
        self.range(start, end)
    }

    fn range(&self, start: i64, end: i64) -> rusqlite::Result<Vec<Session>> {
        let mut query = self.0.prepare(
            "SELECT apps.path, apps.name, apps.alias, app_groups.name, MAX(sessions.start, ?1), MIN(sessions.end, ?2),
                    sessions.id, sessions.end - sessions.start
             FROM sessions JOIN apps ON apps.path = sessions.app
             LEFT JOIN app_groups ON app_groups.app = apps.path
             WHERE sessions.end > ?1 AND sessions.start < ?2
             ORDER BY sessions.start DESC, sessions.id DESC",
        )?;
        query
            .query_map(params![start, end], |row| {
                Ok(Session {
                    app: read_app(row)?,
                    start: row.get(4)?,
                    end: row.get(5)?,
                    id: row.get(6)?,
                    full_duration: row.get(7)?,
                })
            })?
            .collect()
    }

    pub fn first_day(&self) -> rusqlite::Result<Option<NaiveDate>> {
        let first: Option<i64> = self
            .0
            .query_row(
                "SELECT start FROM sessions ORDER BY start LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(first
            .and_then(|start| Local.timestamp_millis_opt(start).single())
            .map(|time| time.date_naive()))
    }
}

fn read_app(row: &rusqlite::Row<'_>) -> rusqlite::Result<Application> {
    Ok(Application {
        id: AppId {
            key: row.get(0)?,
            name: row.get(1)?,
        },
        alias: row.get(2)?,
        group: row.get(3)?,
    })
}

fn day_start(date: NaiveDate) -> Option<i64> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    (0..1440).find_map(|minute| {
        Local
            .from_local_datetime(&(midnight + chrono::Duration::minutes(minute)))
            .earliest()
            .map(|time| time.timestamp_millis())
    })
}

fn day_bounds(date: NaiveDate) -> Option<(i64, i64)> {
    Some((
        day_start(date)?,
        day_start(date.checked_add_days(Days::new(1))?)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_extend_clip_and_keep_the_current_alias() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let app = AppId {
            key: "editor.exe".into(),
            name: "editor".into(),
        };
        let id = store.record(&app, 1000, 2000, None, None, None).unwrap();
        assert_eq!(
            store
                .record(&app, 2000, 3000, Some(id), None, None)
                .unwrap(),
            id
        );
        store.record(&app, 4000, 5000, None, None, None).unwrap();
        store.save_alias(&app, " Writing ").unwrap();
        let sessions = store.range(1500, 4500).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!((sessions[0].start, sessions[0].end), (4000, 4500));
        assert_eq!((sessions[1].start, sessions[1].end), (1500, 3000));
        assert_eq!(sessions[1].app.label(), "Writing");
        assert!(sessions[1].meets_cutoff(2));
        assert!(!sessions[1].meets_cutoff(3));
        store.save_alias(&app, "").unwrap();
        assert_eq!(store.range(1500, 4500).unwrap()[1].app.label(), "editor");
    }

    #[test]
    fn chrome_page_changes_keep_one_foreground_session() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let chrome = AppId {
            key: "chrome.exe".into(),
            name: "chrome".into(),
        };
        let id = store
            .record(&chrome, 1000, 2000, None, Some("First page"), None)
            .unwrap();
        store
            .record(&chrome, 2000, 3000, Some(id), Some("First page"), None)
            .unwrap();
        store
            .record(&chrome, 3000, 4000, Some(id), Some("Second page"), None)
            .unwrap();
        let sessions = store.range(1500, 3500).unwrap();
        assert_eq!(sessions.len(), 1);
        let pages = store.pages(&sessions[0]).unwrap();
        assert_eq!(
            pages
                .iter()
                .map(|p| (p.title.as_str(), p.start, p.end))
                .collect::<Vec<_>>(),
            vec![("First page", 1500, 3000), ("Second page", 3000, 3500)]
        );
        assert_eq!(store.cutoff().unwrap(), 5);
        store.save_cutoff(8).unwrap();
        assert_eq!(store.cutoff().unwrap(), 8);
        assert!(!sessions[0].meets_cutoff(store.cutoff().unwrap()));
        assert_eq!(store.range(1500, 3500).unwrap().len(), 1);
    }
}
