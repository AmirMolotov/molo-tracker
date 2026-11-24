use rusqlite::{OptionalExtension, params};

use super::{BTreeMap, Store, local_device_name};
use crate::platform::AppId;
use crate::sync::{Delta, Exchange, Preferences, Span};

pub(crate) struct SyncProfile {
    pub device: String,
    pub server: String,
    pub username: String,
    pub secret: Option<Vec<u8>>,
    pub cursor: i64,
}

impl Store {
    pub(super) fn initialize_sync(&mut self) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS sync_profile (
                 id INTEGER PRIMARY KEY CHECK(id=1), device TEXT NOT NULL,
                 server TEXT NOT NULL DEFAULT '', username TEXT NOT NULL DEFAULT '',
                 secret BLOB, cursor INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS sync_pending (
                 session INTEGER PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                 end INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sync_import (
                 device TEXT NOT NULL, remote INTEGER NOT NULL,
                 session INTEGER UNIQUE NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 PRIMARY KEY(device, remote)
             );
             CREATE TABLE IF NOT EXISTS sync_devices (
                 id TEXT PRIMARY KEY, name TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sync_settings (
                 id INTEGER PRIMARY KEY CHECK(id=1), baseline TEXT, seed INTEGER NOT NULL DEFAULT 1
             );
             INSERT OR IGNORE INTO sync_settings(id) VALUES (1);",
        )?;
        if tx.execute(
            "INSERT OR IGNORE INTO sync_profile(id, device) VALUES (1, ?1)",
            [uuid::Uuid::new_v4().simple().to_string()],
        )? == 1
        {
            tx.execute("INSERT INTO sync_pending SELECT id, end FROM sessions", [])?;
        }
        tx.commit()
    }

    pub fn sync_profile(&self) -> rusqlite::Result<SyncProfile> {
        self.0.query_row(
            "SELECT device, server, username, secret, cursor FROM sync_profile WHERE id=1",
            [],
            |row| {
                Ok(SyncProfile {
                    device: row.get(0)?,
                    server: row.get(1)?,
                    username: row.get(2)?,
                    secret: row.get(3)?,
                    cursor: row.get(4)?,
                })
            },
        )
    }

    pub fn bind_sync(
        &mut self,
        server: &str,
        username: &str,
        secret: &[u8],
    ) -> rusqlite::Result<()> {
        let old = self.sync_profile()?;
        let tx = self.0.transaction()?;
        if old.server != server || old.username != username {
            tx.execute(
                "DELETE FROM sessions WHERE id IN (SELECT session FROM sync_import)",
                [],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO sync_pending SELECT id, end FROM sessions",
                [],
            )?;
            tx.execute("DELETE FROM sync_devices", [])?;
            tx.execute(
                "UPDATE sync_settings SET baseline=NULL, seed=?1 WHERE id=1",
                [old.username.is_empty()],
            )?;
            tx.execute("UPDATE sync_profile SET cursor=0", [])?;
        }
        tx.execute(
            "UPDATE sync_profile SET server=?1, username=?2, secret=?3",
            params![server, username, secret],
        )?;
        tx.commit()
    }

    pub fn clear_sync_login(&mut self) -> rusqlite::Result<()> {
        let tx = self.0.transaction()?;
        tx.execute(
            "DELETE FROM sessions WHERE id IN (SELECT session FROM sync_import)",
            [],
        )?;
        tx.execute("DELETE FROM sync_devices", [])?;
        tx.execute("UPDATE sync_profile SET secret=NULL, cursor=0", [])?;
        tx.commit()
    }

    pub fn pending_sync(&self, cursor: i64) -> rusqlite::Result<Exchange> {
        let mut query = self.0.prepare(
            "SELECT sessions.id, apps.name, sessions.start, sync_pending.end
             FROM sync_pending JOIN sessions ON sessions.id=sync_pending.session
             JOIN apps ON apps.path=sessions.app ORDER BY sessions.id LIMIT 100",
        )?;
        let mut pending = Vec::new();
        let mut bytes = 128;
        for item in query.query_map([], |row| {
            Ok(Delta {
                device: String::new(),
                id: row.get(0)?,
                app: row.get(1)?,
                start: row.get(2)?,
                end: row.get(3)?,
                pages: Vec::new(),
                domains: Vec::new(),
            })
        })? {
            let mut item = item?;
            item.pages = self.sync_spans(
                "SELECT title, start, end FROM chrome_pages WHERE session_id=?1 ORDER BY start",
                item.id,
            )?;
            item.domains = self.sync_spans(
                "SELECT domain, start, end FROM website_focus WHERE session_id=?1 ORDER BY start",
                item.id,
            )?;
            let size = serde_json::to_vec(&item).map_or(2_000_000, |value| value.len());
            if !pending.is_empty() && bytes + size > 1_900_000 {
                break;
            }
            // A long-running browser session may accumulate enough detail to exceed one request.
            // Its total duration can still sync, alongside the most recent spans that fit.
            while item.pages.len() + item.domains.len() > 0
                && (item.pages.len() > 10000
                    || item.domains.len() > 10000
                    || serde_json::to_vec(&item).map_or(2_000_000, |value| value.len()) > 1_900_000)
            {
                let pages = item.pages.len();
                let domains = item.domains.len();
                if pages >= domains {
                    item.pages.drain(..pages.div_ceil(2));
                } else {
                    item.domains.drain(..domains.div_ceil(2));
                }
            }
            bytes += size + 1;
            pending.push(item);
        }
        let (local_preferences, preferences) = self.prepare_preferences()?;
        Ok(Exchange {
            cursor,
            sessions: pending,
            device_name: local_device_name(),
            preferences,
            local_preferences,
        })
    }

    fn sync_spans(&self, sql: &str, id: i64) -> rusqlite::Result<Vec<Span>> {
        let mut query = self.0.prepare(sql)?;
        query
            .query_map([id], |row| {
                Ok(Span {
                    value: row.get(0)?,
                    start: row.get(1)?,
                    end: row.get(2)?,
                })
            })?
            .collect()
    }

    pub fn apply_sync(
        &mut self,
        sent: &[(i64, i64)],
        cursor: i64,
        sessions: &[Delta],
        devices: &BTreeMap<String, String>,
        preferences: Option<(&Preferences, &Preferences)>,
    ) -> rusqlite::Result<bool> {
        let tx = self.0.transaction()?;
        let mut changed = !sessions.is_empty();
        for (id, name) in devices {
            let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
            if !name.trim().is_empty() {
                changed |= tx.execute(
                    "INSERT INTO sync_devices VALUES (?1,?2) ON CONFLICT(id)
                    DO UPDATE SET name=excluded.name WHERE sync_devices.name<>excluded.name",
                    params![id, name],
                )? != 0;
            }
        }
        for (session, end) in sent {
            tx.execute(
                "DELETE FROM sync_pending WHERE session=?1 AND end=?2",
                params![session, end],
            )?;
        }
        for delta in sessions {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT session FROM sync_import WHERE device=?1 AND remote=?2",
                    params![delta.device, delta.id],
                    |row| row.get(0),
                )
                .optional()?;
            let id = if let Some(id) = existing {
                tx.execute(
                    "UPDATE sessions SET end=?2 WHERE id=?1",
                    params![id, delta.end],
                )?;
                tx.execute("DELETE FROM chrome_pages WHERE session_id=?1", [id])?;
                tx.execute("DELETE FROM website_focus WHERE session_id=?1", [id])?;
                id
            } else {
                let key: Option<String> = tx.query_row(
                    "SELECT MIN(path) FROM apps WHERE lower(name)=lower(?1) AND path NOT LIKE 'sync:%' HAVING COUNT(*)=1",
                    [&delta.app], |row| row.get(0)).optional()?;
                let app = AppId {
                    key: key.unwrap_or_else(|| format!("sync:{}", delta.app.to_lowercase())),
                    name: delta.app.clone(),
                };
                Self::register(&tx, &app)?;
                let id: i64 = tx.query_row(
                    "INSERT INTO sessions(app,start,end) VALUES (?1,?2,?3) RETURNING id",
                    params![app.key, delta.start, delta.end],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "INSERT INTO sync_import VALUES (?1,?2,?3)",
                    params![delta.device, delta.id, id],
                )?;
                id
            };
            for page in &delta.pages {
                tx.execute(
                    "INSERT INTO chrome_pages(session_id,title,start,end) VALUES (?1,?2,?3,?4)",
                    params![id, page.value, page.start, page.end],
                )?;
            }
            for site in &delta.domains {
                tx.execute(
                    "INSERT INTO website_focus(session_id,domain,start,end) VALUES (?1,?2,?3,?4)",
                    params![id, site.value, site.start, site.end],
                )?;
            }
        }
        tx.execute("UPDATE sync_profile SET cursor=?1", [cursor])?;
        if let Some((before, remote)) = preferences {
            changed |= Self::merge_preferences(&tx, before, remote)?;
        }
        tx.commit()?;
        Ok(changed)
    }
}
