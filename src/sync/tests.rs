use super::*;
use crate::platform::AppId;
use chrono::{Local, TimeZone};

#[test]
fn retry_delay_doubles_caps_and_resets_after_success() {
    let mut store = Store::open(Path::new(":memory:")).unwrap();
    let mut client = Client::new(String::new(), String::new(), Some("test-token".into()));
    let complete = |client: &mut Client, store: &mut Store, response| {
        let (sender, receiver) = channel();
        sender
            .send(ResultEvent::Sync {
                sent: Vec::new(),
                local_preferences: Preferences::new(),
                response,
            })
            .unwrap_or_else(|_| panic!("send sync result"));
        client.task = Some(receiver);
        client.tick(store, 10.0);
    };
    let failure = || {
        Err(HttpError {
            status: None,
            message: "Offline.".into(),
        })
    };
    for delay in [30_u32, 60, 120, 240, 300, 300] {
        complete(&mut client, &mut store, failure());
        assert_eq!(client.next, 10.0 + f64::from(delay));
    }
    client.sync_now();
    assert_eq!(client.next, 0.0);
    complete(
        &mut client,
        &mut store,
        Ok(Download {
            cursor: 0,
            sessions: Vec::new(),
            more: false,
            devices: BTreeMap::new(),
            preferences: None,
        }),
    );
    assert_eq!(client.next, 25.0);
    complete(&mut client, &mut store, failure());
    assert_eq!(client.next, 40.0);
}

#[test]
fn local_preference_edit_survives_an_in_flight_download() {
    let mut store = Store::open(Path::new(":memory:")).unwrap();
    let before = store.pending_sync(0).unwrap().local_preferences;
    store.save_cutoff(9).unwrap();
    let remote = Preferences::from([
        ("idle-seconds".into(), 90.into()),
        ("cutoff".into(), 8.into()),
    ]);
    store
        .apply_sync(&[], 0, &[], &BTreeMap::new(), Some((&before, &remote)))
        .unwrap();
    assert_eq!(store.cutoff().unwrap(), 9);
    assert_eq!(store.idle_seconds().unwrap(), 90);
    let next = store.pending_sync(0).unwrap().preferences;
    assert_eq!(next.changes.len(), 1);
    assert_eq!(next.changes[0].key, "cutoff");
    assert_eq!(next.changes[0].expected, 8);
    assert_eq!(next.changes[0].value, 9);
}

#[test]
fn moves_the_former_default_without_overwriting_custom_servers() {
    let path = std::env::temp_dir().join(format!("molo-sync-{}.toml", uuid::Uuid::new_v4()));
    let mut store = Store::open(Path::new(":memory:")).unwrap();
    fs::write(&path, format!("server_url = {FORMER_DEFAULT_SERVER:?}\n")).unwrap();
    let client = Client::open(&path, &mut store).unwrap();
    assert_eq!(client.server, "https://tracker.momentummania.com");
    assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);

    let custom = "server_url = \"https://example.com/sync\"\n";
    fs::write(&path, custom).unwrap();
    let client = Client::open(&path, &mut store).unwrap();
    assert_eq!(client.server, "https://example.com/sync");
    assert_eq!(fs::read_to_string(&path).unwrap(), custom);
    fs::remove_file(path).unwrap();
}

#[test]
fn acknowledging_an_older_upload_keeps_the_newer_recording() {
    let mut store = Store::open(Path::new(":memory:")).unwrap();
    let app = AppId {
        key: "editor.exe".into(),
        name: "Editor".into(),
    };
    let id = store.record(&app, 1000, 2000, None, None, None).unwrap();
    store
        .record(&app, 2000, 3000, Some(id), None, None)
        .unwrap();
    store
        .apply_sync(&[(id, 2000)], 0, &[], &BTreeMap::new(), None)
        .unwrap();
    assert_eq!(store.pending_sync(0).unwrap().sessions[0].end, 3000);
    store
        .apply_sync(&[(id, 3000)], 0, &[], &BTreeMap::new(), None)
        .unwrap();
    assert!(store.pending_sync(0).unwrap().sessions.is_empty());
}

#[test]
#[ignore = "requires a disposable sync server in MOLO_SYNC_TEST_SERVER"]
fn two_devices_exchange_history_through_the_http_api() {
    let server = server_url(&std::env::var("MOLO_SYNC_TEST_SERVER").unwrap()).unwrap();
    let mut first = Store::open(Path::new(":memory:")).unwrap();
    let mut second = Store::open(Path::new(":memory:")).unwrap();
    let mut outsider = Store::open(Path::new(":memory:")).unwrap();
    let username = format!("test-{}", uuid::Uuid::new_v4().simple());
    let other_name = format!("test-{}", uuid::Uuid::new_v4().simple());
    let password = uuid::Uuid::new_v4().to_string();
    let get_token = |store: &mut Store, name: &str, create| {
        let device = store.sync_profile().unwrap().device;
        let login: Login = post(
            &server,
            if create { "register" } else { "login" },
            None,
            &serde_json::json!({ "username": name, "password": password, "device": device }),
        )
        .unwrap_or_else(|error| panic!("{}", error.message));
        let secret = platform::protect(login.token.as_bytes()).unwrap();
        assert_eq!(
            platform::unprotect(&secret).unwrap(),
            login.token.as_bytes()
        );
        store.bind_sync(&server, name, &secret).unwrap();
        login.token
    };
    let first_token = get_token(&mut first, &username, true);
    let second_token = get_token(&mut second, &username, false);
    let other_token = get_token(&mut outsider, &other_name, true);
    let send = |store: &mut Store, token: &str, name: &str| {
        let profile = store.sync_profile().unwrap();
        let mut batch = store.pending_sync(profile.cursor).unwrap();
        batch.device_name = name.to_owned();
        let sent: Vec<_> = batch
            .sessions
            .iter()
            .map(|delta| (delta.id, delta.end))
            .collect();
        let reply: Download = post(&server, "sync", Some(token), &batch)
            .unwrap_or_else(|error| panic!("{}", error.message));
        store
            .apply_sync(
                &sent,
                reply.cursor,
                &reply.sessions,
                &reply.devices,
                reply
                    .preferences
                    .as_ref()
                    .map(|prefs| (&batch.local_preferences, prefs)),
            )
            .unwrap();
        reply.sessions.len()
    };
    let date = Local::now().date_naive();
    let start = Local
        .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp_millis();
    let chrome = AppId {
        key: "c:/private/chrome.exe".into(),
        name: "chrome".into(),
    };
    let editor = AppId {
        key: "editor.exe".into(),
        name: "editor".into(),
    };
    let id = first
        .record(
            &chrome,
            start,
            start + 1000,
            None,
            Some("Testing"),
            Some("example.com"),
        )
        .unwrap();
    second
        .record(&editor, start + 250, start + 750, None, None, None)
        .unwrap();
    first
        .save_groups(std::slice::from_ref(&chrome), "Reading")
        .unwrap();
    first.save_idle_seconds(90).unwrap();
    first.save_alias(&chrome, "Browser").unwrap();
    second.save_cutoff(9).unwrap();

    assert_eq!(send(&mut first, &first_token, "Desktop"), 0);
    assert_eq!(send(&mut second, &second_token, "Laptop"), 1);
    assert_eq!(send(&mut first, &first_token, "Desktop"), 1);
    assert_eq!(send(&mut outsider, &other_token, "Other"), 0);
    assert!(outsider.group_folders().unwrap().is_empty());
    assert_eq!(second.group_folders().unwrap(), ["Reading"]);
    assert_eq!(second.idle_seconds().unwrap(), 90);
    assert_eq!(first.cutoff().unwrap(), 9);
    assert!(
        second
            .apps()
            .unwrap()
            .iter()
            .any(|app| app.label() == "Browser" && app.group.as_deref() == Some("Reading"))
    );
    assert_eq!(first.day_total(date).unwrap(), 1000);
    assert_eq!(second.day_total(date).unwrap(), 1000);
    assert!(outsider.day(date).unwrap().is_empty());
    let tomorrow = date.succ_opt().unwrap();
    for (store, other_name) in [(&first, "Laptop"), (&second, "Desktop")] {
        let app_usage = store.usage(date, tomorrow).unwrap();
        assert_eq!(app_usage.len(), 2);
        assert!(
            app_usage
                .iter()
                .all(|(_, usage)| usage.time == 500 && usage.sessions == 1)
        );
        assert_eq!(store.website_time(date, tomorrow).unwrap()[0].chrome, 500);
        let per_device = store.devices(date, tomorrow).unwrap();
        assert_eq!(
            per_device.iter().map(|d| d.time).collect::<Vec<_>>(),
            [1000, 500]
        );
        assert!(
            per_device
                .iter()
                .any(|d| !d.device.local && d.device.name == other_name)
        );
    }
    assert_eq!(second.page_titles(date).unwrap()[0].1, "Testing");

    first
        .record(
            &chrome,
            start + 1000,
            start + 2000,
            Some(id),
            Some("Testing"),
            Some("example.com"),
        )
        .unwrap();
    assert!(first.change_folder("reading", Some("Research")).unwrap());
    assert_eq!(send(&mut first, &first_token, "Desktop"), 0);
    assert_eq!(send(&mut second, &second_token, "Laptop"), 1);
    assert_eq!(send(&mut second, &second_token, "Laptop"), 0);
    assert_eq!(second.day_total(date).unwrap(), 2000);
    assert_eq!(second.group_folders().unwrap(), ["Research"]);
    assert!(second.change_folder("research", None).unwrap());
    send(&mut second, &second_token, "Laptop");
    send(&mut first, &first_token, "Desktop");
    assert!(first.group_folders().unwrap().is_empty());
    first.save_cutoff(12).unwrap();
    second.save_cutoff(19).unwrap();
    send(&mut first, &first_token, "Desktop");
    send(&mut second, &second_token, "Laptop");
    assert_eq!(second.cutoff().unwrap(), 12);
    assert!(second.pending_sync(0).unwrap().sessions.is_empty());
    second.clear_sync_login().unwrap();
    assert_eq!(second.day_total(date).unwrap(), 500);
}
