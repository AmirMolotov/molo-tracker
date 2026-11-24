use std::collections::{BTreeMap, BTreeSet};

use super::{DeviceUsage, Session};

pub(crate) struct Covered<'a> {
    pub session: &'a Session,
    pub start: i64,
    pub end: i64,
}

pub(crate) fn covered(sessions: &[Session]) -> Vec<Covered<'_>> {
    let mut events = Vec::with_capacity(sessions.len() * 2);
    for (index, session) in sessions.iter().enumerate() {
        events.push((session.start, true, index));
        events.push((session.end, false, index));
    }
    events.sort_unstable();

    // The most recently started session owns each slice. The source IDs break ties identically on every PC.
    let mut active = BTreeSet::<(i64, &str, i64, usize)>::new();
    let mut parts: Vec<Covered<'_>> = Vec::new();
    let mut start = events.first().map_or(0, |event| event.0);
    for (at, begins, index) in events {
        if at > start
            && let Some(&(_, _, _, winner)) = active.last()
        {
            let session = &sessions[winner];
            if let Some(last) = parts.last_mut()
                && last.session.id == session.id
                && last.end == start
            {
                last.end = at;
            } else {
                parts.push(Covered {
                    session,
                    start,
                    end: at,
                });
            }
        }
        let session = &sessions[index];
        let priority = (
            session.original_start,
            session.device.id.as_str(),
            session.source_id,
            index,
        );
        if begins {
            active.insert(priority);
        } else {
            active.remove(&priority);
        }
        start = at;
    }
    parts
}

pub(super) fn device_usage(sessions: &[Session]) -> Vec<DeviceUsage> {
    let mut devices = BTreeMap::new();
    for session in sessions {
        let usage = devices
            .entry(&session.device.id)
            .or_insert_with(|| DeviceUsage {
                device: session.device.clone(),
                time: 0,
            });
        usage.time += session.end - session.start;
    }
    let mut devices: Vec<_> = devices.into_values().collect();
    devices.sort_by(|a, b| {
        b.time
            .cmp(&a.time)
            .then_with(|| a.device.name.cmp(&b.device.name))
    });
    devices
}
