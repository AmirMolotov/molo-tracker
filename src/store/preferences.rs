use rusqlite::{Connection, params};
use serde_json::Value;

use super::{DEFAULT_IDLE_SECONDS, Store};
use crate::sync::{PreferenceChange, PreferencePatch, Preferences};

const CUTOFF: &str = "cutoff";
const IDLE: &str = "idle-seconds";

impl Store {
    pub(super) fn preference_paths(
        db: &Connection,
        app: &crate::platform::AppId,
    ) -> rusqlite::Result<Vec<String>> {
        let linked: bool = db.query_row(
            "SELECT username<>'' FROM sync_profile WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        if !linked {
            return Ok(vec![app.key.clone()]);
        }
        let mut query = db.prepare("SELECT path, name FROM apps")?;
        let mut rows = query.query([])?;
        let name = app.name.to_lowercase();
        let mut paths = Vec::new();
        while let Some(row) = rows.next()? {
            if row.get::<_, String>(1)?.to_lowercase() == name {
                paths.push(row.get(0)?);
            }
        }
        Ok(paths)
    }

    pub(super) fn prepare_preferences(&self) -> rusqlite::Result<(Preferences, PreferencePatch)> {
        let (baseline, seed): (Option<String>, bool) = self.0.query_row(
            "SELECT baseline, seed FROM sync_settings WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let first = baseline.is_none();
        let baseline = baseline
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default();
        let local = read_preferences(&self.0)?;
        let changes = if first && !seed {
            Vec::new()
        } else {
            differences(&baseline, &local)
        };
        Ok((
            local,
            PreferencePatch {
                seed: first,
                changes,
            },
        ))
    }

    pub(super) fn merge_preferences(
        db: &Connection,
        before: &Preferences,
        remote: &Preferences,
    ) -> rusqlite::Result<bool> {
        let local = read_preferences(db)?;
        let mut merged = remote.clone();
        for change in differences(before, &local) {
            merged.insert(change.key, change.value);
        }
        merged.retain(|_, value| !value.is_null());
        let changed = merged != local;
        if changed {
            project_preferences(db, &merged)?;
        }
        db.execute(
            "UPDATE sync_settings SET baseline=?1 WHERE id=1",
            [serde_json::to_string(remote)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?],
        )?;
        Ok(changed)
    }

    // Called only when a device discovers a new executable path.
    pub(super) fn inherit_preferences(
        db: &Connection,
        id: &crate::platform::AppId,
    ) -> rusqlite::Result<()> {
        if id.key.starts_with("sync:") {
            return Ok(());
        }
        let source = format!("sync:{}", id.name.to_lowercase());
        db.execute(
            "UPDATE apps SET alias=(SELECT alias FROM apps WHERE path=?2) WHERE path=?1",
            params![id.key, source],
        )?;
        db.execute("INSERT OR IGNORE INTO app_groups(app,name) SELECT ?1,name FROM app_groups WHERE app=?2", params![id.key, source])?;
        db.execute("INSERT OR IGNORE INTO idle_exceptions(app) SELECT ?1 FROM idle_exceptions WHERE app=?2", params![id.key, source])?;
        Ok(())
    }
}

fn differences(before: &Preferences, after: &Preferences) -> Vec<PreferenceChange> {
    before
        .keys()
        .chain(after.keys())
        .collect::<super::BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let expected = before.get(key).unwrap_or(&Value::Null);
            let value = after.get(key).unwrap_or(&Value::Null);
            (expected != value).then(|| PreferenceChange {
                key: key.clone(),
                expected: expected.clone(),
                value: value.clone(),
            })
        })
        .collect()
}

fn read_preferences(db: &Connection) -> rusqlite::Result<Preferences> {
    let mut prefs = Preferences::new();
    let (cutoff, idle): (u32, u32) = db.query_row(
        "SELECT cutoff,idle_seconds FROM settings WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if cutoff != 5 {
        prefs.insert(CUTOFF.into(), cutoff.into());
    }
    if idle != DEFAULT_IDLE_SECONDS {
        prefs.insert(IDLE.into(), idle.into());
    }
    let mut groups =
        db.prepare("SELECT name FROM group_folders UNION SELECT name FROM app_groups")?;
    for group in groups.query_map([], |row| row.get::<_, String>(0))? {
        let name = group?;
        prefs.insert(format!("group/{}", name.to_lowercase()), name.into());
    }
    let mut apps = db.prepare(
        "SELECT apps.name, apps.alias, app_groups.name, idle_exceptions.app IS NOT NULL
         FROM apps LEFT JOIN app_groups ON apps.path=app_groups.app
         LEFT JOIN idle_exceptions ON apps.path=idle_exceptions.app
         WHERE apps.alias IS NOT NULL OR app_groups.app IS NOT NULL OR idle_exceptions.app IS NOT NULL
         ORDER BY apps.path LIKE 'sync:%', apps.path",
    )?;
    let mut rows = apps.query([])?;
    while let Some(row) = rows.next()? {
        let app = row.get::<_, String>(0)?.to_lowercase();
        if let Some(alias) = row.get::<_, Option<String>>(1)? {
            prefs.entry(format!("alias/{app}")).or_insert(alias.into());
        }
        if let Some(group) = row.get::<_, Option<String>>(2)? {
            prefs
                .entry(format!("assignment/{app}"))
                .or_insert(group.into());
        }
        if row.get::<_, bool>(3)? {
            prefs.insert(format!("idle-exception/{app}"), true.into());
        }
    }
    Ok(prefs)
}

fn project_preferences(db: &Connection, merged: &Preferences) -> rusqlite::Result<()> {
    let number = |key, fallback| {
        merged
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(fallback)
    };
    db.execute(
        "UPDATE settings SET cutoff=?1,idle_seconds=?2 WHERE id=1",
        params![number(CUTOFF, 5), number(IDLE, DEFAULT_IDLE_SECONDS)],
    )?;
    db.execute_batch("DELETE FROM group_folders; DELETE FROM app_groups; DELETE FROM idle_exceptions; UPDATE apps SET alias=NULL WHERE alias IS NOT NULL;")?;
    let mut paths_by_app = super::BTreeMap::<String, Vec<String>>::new();
    let mut query = db.prepare("SELECT path,name FROM apps")?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        paths_by_app
            .entry(row.get::<_, String>(1)?.to_lowercase())
            .or_default()
            .push(row.get(0)?);
    }
    for (key, value) in merged {
        let Some((kind, app)) = key.split_once('/') else {
            continue;
        };
        if kind == "group" {
            if let Some(name) = value.as_str() {
                db.execute(
                    "INSERT OR IGNORE INTO group_folders(name) VALUES (?1)",
                    [name],
                )?;
            }
            continue;
        }
        if !matches!(kind, "alias" | "assignment" | "idle-exception") || app.is_empty() {
            continue;
        }
        let key = format!("sync:{app}");
        let paths = paths_by_app.entry(app.to_owned()).or_default();
        if !paths.contains(&key) {
            db.execute(
                "INSERT OR IGNORE INTO apps(path,name) VALUES (?1,?2)",
                params![key, app],
            )?;
            paths.push(key);
        }
        if kind == "assignment"
            && let Some(group) = value.as_str()
        {
            db.execute(
                "INSERT OR IGNORE INTO group_folders(name) VALUES (?1)",
                [group],
            )?;
        }
        for path in paths.iter() {
            match kind {
                "alias" => {
                    db.execute(
                        "UPDATE apps SET alias=?2 WHERE path=?1",
                        params![path, value.as_str()],
                    )?;
                }
                "assignment" if value.is_string() => {
                    db.execute(
                        "INSERT INTO app_groups(app,name) VALUES (?1,?2)",
                        params![path, value.as_str()],
                    )?;
                }
                "idle-exception" if value.as_bool() == Some(true) => {
                    db.execute("INSERT INTO idle_exceptions(app) VALUES (?1)", [path])?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
