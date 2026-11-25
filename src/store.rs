use chrono::{Days, Local, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::platform::AppId;

mod coverage;
mod preferences;
mod reporting;
mod sync;
pub(crate) use coverage::covered;
pub(crate) use reporting::{AppTotals, Website};

pub const DEFAULT_IDLE_SECONDS: u32 = 300;

#[derive(Clone, Debug)]
pub struct Application {
    pub id: AppId,
    pub alias: Option<String>,
    pub group: Option<String>,
    pub last_seen: Option<i64>,
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

    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.label().to_lowercase().contains(query)
            || self.id.name.to_lowercase().contains(query)
            || self
                .group
                .as_deref()
                .is_some_and(|name| name.to_lowercase().contains(query))
    }
}

pub struct Session {
    pub id: i64,
    pub app: Application,
    pub start: i64,
    pub end: i64,
    pub full_duration: i64,
    pub device: Device,
    pub source_id: i64,
    pub original_start: i64,
}

#[derive(Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub local: bool,
}

impl Device {
    fn remote(id: String, name: Option<String>) -> Self {
        let name = name
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Device {}", id.chars().take(8).collect::<String>()));
        Self {
            id,
            name,
            local: false,
        }
    }

    pub fn label(&self) -> String {
        if self.local {
            format!("{} (this PC)", self.name)
        } else {
            self.name.clone()
        }
    }
}

pub struct DeviceUsage {
    pub device: Device,
    pub time: i64,
}

pub(crate) fn local_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .map(|name| {
            name.trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect()
        })
        .unwrap_or_else(|| "Windows PC".into())
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
             CREATE INDEX IF NOT EXISTS sessions_app_end ON sessions(app, end);
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
             CREATE INDEX IF NOT EXISTS website_focus_domain ON website_focus(domain);
             CREATE INDEX IF NOT EXISTS website_focus_session_end ON website_focus(session_id, end);
             CREATE TABLE IF NOT EXISTS website_groups (
                 domain TEXT PRIMARY KEY, name TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS settings (
                 id INTEGER PRIMARY KEY CHECK(id = 1),
                 cutoff INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO settings(id, cutoff) VALUES (1, 5);
             CREATE TABLE IF NOT EXISTS idle_exceptions (
                 app TEXT PRIMARY KEY REFERENCES apps(path)
             );",
        )?;
        let mut migration = db.prepare(
            "SELECT name FROM pragma_table_info('settings') WHERE name = 'idle_seconds'",
        )?;
        if !migration.exists([])? {
            db.execute(
                "ALTER TABLE settings ADD COLUMN idle_seconds INTEGER NOT NULL DEFAULT 300",
                [],
            )?;
        }
        drop(migration);
        let mut store = Self(db);
        store.initialize_sync()?;
        Ok(store)
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

    pub fn idle_seconds(&self) -> rusqlite::Result<u32> {
        self.0.query_row(
            "SELECT idle_seconds FROM settings WHERE id = 1",
            [],
            |row| row.get(0),
        )
    }

    pub fn save_idle_seconds(&self, seconds: u32) -> rusqlite::Result<()> {
        self.0.execute(
            "UPDATE settings SET idle_seconds = ?1 WHERE id = 1",
            [seconds],
        )?;
        Ok(())
    }

    pub fn idle_exceptions(&self) -> rusqlite::Result<BTreeSet<String>> {
        let mut query = self.0.prepare("SELECT app FROM idle_exceptions")?;
        query.query_map([], |row| row.get(0))?.collect()
    }

    pub fn set_idle_exception(&mut self, app: &AppId, enabled: bool) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        if enabled {
            Self::register(&tx, app)?;
        }
        for path in Self::preference_paths(&tx, app)? {
            if enabled {
                tx.execute(
                    "INSERT OR IGNORE INTO idle_exceptions(app) VALUES (?1)",
                    [&path],
                )?;
            } else {
                tx.execute("DELETE FROM idle_exceptions WHERE app = ?1", [&path])?;
            }
        }
        tx.commit()
    }

    fn register(db: &Connection, app: &AppId) -> rusqlite::Result<()> {
        let inserted = db.execute(
            "INSERT OR IGNORE INTO apps(path, name) VALUES(?1, ?2)",
            params![app.key, app.name],
        )?;
        if inserted != 0 {
            Self::inherit_preferences(db, app)?;
        }
        Ok(())
    }

    pub fn save_alias(&mut self, app: &AppId, value: &str) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        Self::register(&tx, app)?;
        let alias = Some(value.trim()).filter(|alias| !alias.is_empty());
        for path in Self::preference_paths(&tx, app)? {
            tx.execute(
                "UPDATE apps SET alias = ?2 WHERE path = ?1",
                params![path, alias],
            )?;
        }
        tx.commit()
    }

    pub fn apps(&self) -> rusqlite::Result<Vec<Application>> {
        let mut query = self.0.prepare(
            "SELECT apps.path, apps.name, apps.alias, app_groups.name,
                    (SELECT MAX(end) FROM sessions
                     WHERE app IN (apps.path, 'sync:' || lower(apps.name)))
             FROM apps LEFT JOIN app_groups ON app_groups.app = apps.path
             WHERE apps.path NOT LIKE 'sync:%' OR NOT EXISTS (
                 SELECT 1 FROM apps AS origin WHERE origin.path NOT LIKE 'sync:%'
                 AND lower(origin.name)=lower(apps.name)
             )",
        )?;
        query
            .query_map([], |row| {
                let mut app = read_app(row)?;
                app.last_seen = row.get(4)?;
                Ok(app)
            })?
            .collect()
    }

    pub fn group_folders(&self) -> rusqlite::Result<Vec<String>> {
        Self::folder_names(&self.0)
    }

    fn folder_names(db: &Connection) -> rusqlite::Result<Vec<String>> {
        let mut query = db.prepare(
            "SELECT name FROM group_folders UNION SELECT name FROM app_groups
             UNION SELECT name FROM website_groups WHERE name <> ''",
        )?;
        query.query_map([], |row| row.get(0))?.collect()
    }

    pub fn change_folder(&mut self, key: &str, new_name: Option<&str>) -> rusqlite::Result<bool> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let names = Self::folder_names(&tx)?;
        if let Some(name) = new_name {
            let new_key = name.to_lowercase();
            if new_key != key && names.iter().any(|name| name.to_lowercase() == new_key) {
                return Ok(false);
            }
        }
        for name in names.iter().filter(|name| name.to_lowercase() == key) {
            tx.execute("DELETE FROM group_folders WHERE name = ?1", [name])?;
            if let Some(new_name) = new_name {
                tx.execute(
                    "UPDATE app_groups SET name = ?2 WHERE name = ?1",
                    params![name, new_name],
                )?;
                tx.execute(
                    "UPDATE website_groups SET name = ?2 WHERE name = ?1",
                    params![name, new_name],
                )?;
            } else {
                tx.execute("DELETE FROM app_groups WHERE name = ?1", [name])?;
                tx.execute("DELETE FROM website_groups WHERE name = ?1", [name])?;
            }
        }
        if let Some(name) = new_name {
            tx.execute("INSERT INTO group_folders(name) VALUES (?1)", [name])?;
        }
        tx.commit()?;
        Ok(true)
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
            for path in Self::preference_paths(&tx, app)? {
                if name.is_empty() {
                    tx.execute("DELETE FROM app_groups WHERE app = ?1", [&path])?;
                } else {
                    tx.execute(
                        "INSERT INTO app_groups(app, name) VALUES(?1, ?2)
                         ON CONFLICT(app) DO UPDATE SET name = excluded.name",
                        params![path, name],
                    )?;
                }
            }
        }
        tx.commit()
    }

    pub fn report_devices(&self) -> rusqlite::Result<Vec<Device>> {
        let id = self
            .0
            .query_row("SELECT device FROM sync_profile WHERE id=1", [], |row| {
                row.get(0)
            })?;
        let mut devices = vec![Device {
            id,
            name: local_device_name(),
            local: true,
        }];
        let mut query = self.0.prepare(
            "SELECT DISTINCT sync_import.device, sync_devices.name
             FROM sync_import LEFT JOIN sync_devices ON sync_devices.id = sync_import.device
             ORDER BY sync_devices.name COLLATE NOCASE, sync_import.device",
        )?;
        for device in query.query_map([], |row| Ok(Device::remote(row.get(0)?, row.get(1)?)))? {
            devices.push(device?);
        }
        Ok(devices)
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
        tx.execute(
            "INSERT INTO sync_pending(session, end) VALUES (?1, ?2)
                    ON CONFLICT(session) DO UPDATE SET end = excluded.end",
            params![id, end],
        )?;
        tx.commit()?;
        Ok(id)
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

    pub fn page_titles(&self, day: NaiveDate) -> rusqlite::Result<Vec<(i64, String)>> {
        let Some((start, end)) = day_bounds(day) else {
            return Ok(Vec::new());
        };
        let mut query = self.0.prepare(
            "SELECT DISTINCT sessions.id, chrome_pages.title
             FROM sessions JOIN chrome_pages ON chrome_pages.session_id = sessions.id
             WHERE sessions.end > ?1 AND sessions.start < ?2
               AND chrome_pages.end > ?1 AND chrome_pages.start < ?2
             ORDER BY sessions.id, chrome_pages.title",
        )?;
        query
            .query_map(params![start, end], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    pub fn day_total(&self, date: NaiveDate) -> rusqlite::Result<i64> {
        let Some((start, end)) = day_bounds(date) else {
            return Ok(0);
        };
        self.0.query_row(
            "WITH intervals AS (
                 SELECT MAX(start, ?1) AS start, MIN(end, ?2) AS end, id
                 FROM sessions WHERE end > ?1 AND start < ?2
             ), coverage AS (
                 SELECT start, end, MAX(end) OVER (
                     ORDER BY start, id ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
                 ) AS prior_end FROM intervals
             )
             SELECT COALESCE(SUM(MAX(0, end - MAX(start, COALESCE(prior_end, start)))), 0)
             FROM coverage",
            params![start, end],
            |row| row.get(0),
        )
    }

    pub fn day(&self, date: NaiveDate) -> rusqlite::Result<Vec<Session>> {
        let Some((start, end)) = day_bounds(date) else {
            return Ok(Vec::new());
        };
        self.range(start, end, None)
    }

    fn range(&self, start: i64, end: i64, device: Option<&str>) -> rusqlite::Result<Vec<Session>> {
        let local = local_device_name();
        let mut query = self.0.prepare(
            "SELECT apps.path, apps.name, apps.alias, app_groups.name, MAX(sessions.start, ?1), MIN(sessions.end, ?2),
                    sessions.id, sessions.end - sessions.start, sessions.start,
                    sync_import.device, sync_import.remote, sync_devices.name, sync_profile.device
             FROM sessions JOIN apps ON apps.path = sessions.app
             CROSS JOIN sync_profile
             LEFT JOIN app_groups ON app_groups.app = apps.path
             LEFT JOIN sync_import ON sync_import.session = sessions.id
             LEFT JOIN sync_devices ON sync_devices.id = sync_import.device
             WHERE sessions.end > ?1 AND sessions.start < ?2
               AND (?3 IS NULL OR COALESCE(sync_import.device, sync_profile.device) = ?3)
             ORDER BY sessions.start DESC, sessions.id DESC",
        )?;
        query
            .query_map(params![start, end, device], |row| {
                let remote: Option<String> = row.get(9)?;
                let device = match remote {
                    Some(id) => Device::remote(id, row.get(11)?),
                    None => Device {
                        id: row.get(12)?,
                        name: local.clone(),
                        local: true,
                    },
                };
                Ok(Session {
                    app: read_app(row)?,
                    start: row.get(4)?,
                    end: row.get(5)?,
                    id: row.get(6)?,
                    full_duration: row.get(7)?,
                    original_start: row.get(8)?,
                    source_id: row.get::<_, Option<i64>>(10)?.unwrap_or(row.get(6)?),
                    device,
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
        last_seen: None,
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
    fn idle_exception_can_be_added_for_an_unrecorded_app_and_removed() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let app = AppId {
            key: "player.exe".into(),
            name: "Player".into(),
        };
        assert!(store.idle_exceptions().unwrap().is_empty());
        store.set_idle_exception(&app, true).unwrap();
        assert_eq!(
            store.idle_exceptions().unwrap(),
            BTreeSet::from([app.key.clone()])
        );
        assert_eq!(store.apps().unwrap()[0].label(), "Player");
        store.set_idle_exception(&app, false).unwrap();
        assert!(store.idle_exceptions().unwrap().is_empty());
    }

    #[test]
    fn editing_folders_preserves_sessions_and_handles_empty_folders() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let app = AppId {
            key: "editor.exe".into(),
            name: "Editor".into(),
        };
        store
            .save_groups(std::slice::from_ref(&app), "Écriture")
            .unwrap();
        store.save_groups(&[], "Work").unwrap();
        store.record(&app, 1000, 2000, None, None, None).unwrap();

        assert!(!store.change_folder("écriture", Some("work")).unwrap());
        assert!(store.change_folder("écriture", Some("ÉCRITURE")).unwrap());
        assert_eq!(store.apps().unwrap()[0].group.as_deref(), Some("ÉCRITURE"));
        assert!(store.change_folder("work", Some("Projects")).unwrap());
        assert!(store.change_folder("écriture", None).unwrap());
        assert_eq!(store.group_folders().unwrap(), ["Projects"]);
        let sessions = store.range(1000, 2000, None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].app.group.is_none());
    }

    #[test]
    fn idle_setting_can_be_changed_or_disabled() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        assert_eq!(store.idle_seconds().unwrap(), DEFAULT_IDLE_SECONDS);
        assert_eq!(store.cutoff().unwrap(), 5);
        store.save_idle_seconds(45).unwrap();
        assert_eq!(store.idle_seconds().unwrap(), 45);
        store.save_idle_seconds(0).unwrap();
        assert_eq!(store.idle_seconds().unwrap(), 0);
        assert_eq!(store.cutoff().unwrap(), 5);
    }

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
        let sessions = store.range(1500, 4500, None).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!((sessions[0].start, sessions[0].end), (4000, 4500));
        assert_eq!((sessions[1].start, sessions[1].end), (1500, 3000));
        assert_eq!(sessions[1].app.label(), "Writing");
        assert!(sessions[1].meets_cutoff(2));
        assert!(!sessions[1].meets_cutoff(3));
        store.save_alias(&app, "").unwrap();
        assert_eq!(
            store.range(1500, 4500, None).unwrap()[1].app.label(),
            "editor"
        );
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
        let sessions = store.range(1500, 3500, None).unwrap();
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
        assert_eq!(store.range(1500, 3500, None).unwrap().len(), 1);
    }
}
