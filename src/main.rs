#![windows_subsystem = "windows"]

mod platform;
mod report;
mod store;
mod ui;
mod websites;

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use chrono::{Datelike, Days, Local, Months, NaiveDate, Utc};
use platform::{AppId, FocusedApp};
use report::{Period, Report};
use rune::{
    App, AppSpec, Image, ImageCache, TextEdit, Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, Tx,
};
use store::{Application, BrowserPage, Session, Store};
use websites::WebsiteProbe;

struct Sample {
    app: Option<FocusedApp>,
    time: i64,
}

struct DragPreview {
    source: String,
    point: [f32; 2],
    target: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Activity,
    Calendar,
    Reports,
    Settings,
}

struct State {
    store: Store,
    live: bool,
    paused: bool,
    today: NaiveDate,
    day: NaiveDate,
    first: NaiveDate,
    sample: Option<Sample>,
    website_probe: Option<WebsiteProbe>,
    continuing: Option<i64>,
    next_sample: f64,
    next_scan: f64,
    open_apps: Vec<AppId>,
    icons: Option<ImageCache>,
    app_icons: RefCell<HashMap<String, Image>>,
    sessions: Vec<Session>,
    prior_total: i64,
    month: NaiveDate,
    month_totals: [i64; 31],
    expanded: Option<i64>,
    pages: Vec<BrowserPage>,
    apps: Vec<Application>,
    screen: Screen,
    cutoff: u32,
    cutoff_input: TextEdit,
    cutoff_notice: Option<String>,
    selected: Option<AppId>,
    selection: BTreeSet<String>,
    selection_anchor: Option<String>,
    folders: BTreeMap<String, String>,
    folder: Option<String>,
    drag_preview: Option<DragPreview>,
    alias: TextEdit,
    group_input: TextEdit,
    report_period: Period,
    report_date: NaiveDate,
    report: Report,
    report_group: Option<String>,
    report_websites: bool,
    notice: Option<String>,
    tracking_error: Option<String>,
}

impl State {
    fn new(store: Store, live: bool) -> Self {
        let today = Local::now().date_naive();
        let (cutoff, cutoff_notice) = match store.cutoff() {
            Ok(seconds) => (seconds, None),
            Err(error) => (5, Some(format!("Could not read the cutoff. {error}"))),
        };
        let mut state = Self {
            store,
            live,
            today,
            day: today,
            first: today,
            paused: false,
            sample: None,
            website_probe: if live {
                WebsiteProbe::start().ok()
            } else {
                None
            },
            continuing: None,
            next_sample: 0.0,
            next_scan: 0.0,
            open_apps: Vec::new(),
            icons: None,
            app_icons: RefCell::default(),
            sessions: Vec::new(),
            prior_total: 0,
            month: today.with_day(1).unwrap(),
            month_totals: [0; 31],
            expanded: None,
            pages: Vec::new(),
            apps: Vec::new(),
            screen: Screen::Activity,
            cutoff,
            cutoff_input: TextEdit::new(cutoff.to_string()),
            cutoff_notice,
            selected: None,
            selection: BTreeSet::new(),
            selection_anchor: None,
            folders: BTreeMap::new(),
            folder: None,
            drag_preview: None,
            alias: TextEdit::new(""),
            group_input: TextEdit::new(""),
            report_period: Period::Daily,
            report_date: today,
            report: Report::default(),
            report_group: None,
            report_websites: false,
            notice: None,
            tracking_error: None,
        };
        state.refresh();
        state
    }

    fn refresh(&mut self) {
        match self.store.day(self.day) {
            Ok(sessions) => self.sessions = sessions,
            Err(error) => self.notice = Some(format!("Could not read activity. {error}")),
        }
        match self.store.first_day() {
            Ok(first) => self.first = first.unwrap_or(self.today),
            Err(error) => self.notice = Some(format!("Could not read activity dates. {error}")),
        }
        if let Some(previous) = self.day.checked_sub_days(Days::new(1)) {
            match self.store.day_total(previous) {
                Ok(total) => self.prior_total = total,
                Err(error) => {
                    self.notice = Some(format!("Could not read the previous day. {error}"))
                }
            }
        }
        self.refresh_pages();
        if self.screen == Screen::Reports {
            self.refresh_report();
        }
    }

    fn refresh_pages(&mut self) {
        self.pages.clear();
        if let Some(session) = self.sessions.iter().find(|s| Some(s.id) == self.expanded) {
            match self.store.pages(session) {
                Ok(pages) => self.pages = pages,
                Err(error) => {
                    self.notice = Some(format!("Could not read browser page titles. {error}"))
                }
            }
        }
    }

    fn select_day(&mut self, day: NaiveDate) {
        self.day = day;
        self.expanded = None;
        self.refresh();
    }

    fn show_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.notice = None;
        match screen {
            Screen::Activity => self.refresh(),
            Screen::Calendar => self.show_month(self.day.with_day(1).unwrap()),
            Screen::Reports => self.refresh_report(),
            Screen::Settings => {
                if self.live {
                    self.open_apps = platform::open_apps();
                }
                self.refresh_apps();
            }
        }
    }

    fn refresh_report(&mut self) {
        match Report::load(&self.store, self.report_period, self.report_date) {
            Ok(report) => self.report = report,
            Err(error) => self.notice = Some(format!("Could not read the report. {error}")),
        }
    }

    fn step_report(&mut self, next: bool) {
        let start = self.report_period.start(self.report_date);
        if let Some(date) = self.report_period.adjacent(start, next) {
            self.report_date = if date == self.report_period.start(self.today) {
                self.today
            } else {
                date
            };
            self.report_group = None;
            self.refresh_report();
        }
    }

    fn show_month(&mut self, month: NaiveDate) {
        self.month = month;
        self.month_totals = [0; 31];
        let today = self.today;
        for day in month
            .iter_days()
            .take_while(|day| day.month() == month.month() && *day <= today)
        {
            self.refresh_calendar_day(day);
        }
    }

    fn refresh_calendar_day(&mut self, day: NaiveDate) {
        if day.with_day(1) == Some(self.month) {
            match self.store.day_total(day) {
                Ok(total) => self.month_totals[day.day0() as usize] = total,
                Err(error) => {
                    self.notice = Some(format!("Could not read calendar activity. {error}"))
                }
            }
        }
    }

    fn step_month(&mut self, forward: bool) {
        let step = Months::new(1);
        let month = if forward {
            self.month.checked_add_months(step)
        } else {
            self.month.checked_sub_months(step)
        };
        if let Some(month) = month {
            self.show_month(month);
        }
    }

    fn save_cutoff(&mut self) {
        let Ok(seconds) = self.cutoff_input.text.trim().parse::<u32>() else {
            self.cutoff_notice = Some("Enter a nonnegative whole number of seconds.".into());
            return;
        };
        match self.store.save_cutoff(seconds) {
            Ok(()) => {
                self.cutoff = seconds;
                self.cutoff_notice = Some("Cutoff saved.".into());
            }
            Err(error) => self.cutoff_notice = Some(format!("Could not save the cutoff. {error}")),
        }
    }

    fn refresh_apps(&mut self) {
        match self.store.apps() {
            Ok(mut apps) => {
                for id in &self.open_apps {
                    if !apps.iter().any(|app| app.id.key == id.key) {
                        apps.push(Application {
                            id: id.clone(),
                            alias: None,
                            group: None,
                        });
                    }
                }
                apps.sort_by_cached_key(|app| (app.label().to_lowercase(), app.id.key.clone()));
                self.apps = apps;
            }
            Err(error) => self.notice = Some(format!("Could not read apps. {error}")),
        }
        match self.store.group_folders() {
            Ok(folders) => {
                self.folders = folders
                    .into_iter()
                    .map(|name| (name.to_lowercase(), name))
                    .collect()
            }
            Err(error) => self.notice = Some(format!("Could not read folders. {error}")),
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.sample = None;
        self.continuing = None;
    }

    fn visible_apps(&self) -> impl Iterator<Item = &Application> {
        self.apps.iter().filter(|app| {
            self.folder
                .as_ref()
                .is_none_or(|folder| report::group_key(app) == *folder)
        })
    }

    fn open_folder(&mut self, folder: Option<String>) {
        self.folder = folder;
        self.selection.clear();
        self.selection_anchor = None;
        self.update_editor();
    }

    fn select(&mut self, key: &str, ctrl: bool, shift: bool) {
        if shift && let Some(anchor) = &self.selection_anchor {
            let visible: Vec<_> = self.visible_apps().map(|app| app.id.key.clone()).collect();
            if let (Some(from), Some(to)) = (
                visible.iter().position(|item| item == anchor),
                visible.iter().position(|item| item == key),
            ) {
                if !ctrl {
                    self.selection.clear();
                }
                self.selection
                    .extend(visible[from.min(to)..=from.max(to)].iter().cloned());
                self.update_editor();
                return;
            }
        }
        if !ctrl {
            self.selection.clear();
        }
        if !ctrl || !self.selection.remove(key) {
            self.selection.insert(key.to_owned());
        }
        self.selection_anchor = Some(key.to_owned());
        self.update_editor();
    }

    fn update_editor(&mut self) {
        let app = if self.selection.len() == 1 {
            self.apps
                .iter()
                .find(|app| self.selection.contains(&app.id.key))
        } else {
            None
        };
        self.selected = app.map(|app| app.id.clone());
        self.alias = TextEdit::new(app.and_then(|app| app.alias.as_deref()).unwrap_or(""));
        self.group_input = TextEdit::new(app.and_then(|app| app.group.as_deref()).unwrap_or(""));
        self.notice = None;
    }

    fn save_group(&mut self) {
        self.move_to_group(&self.group_input.text.trim().to_owned());
    }

    fn move_to_group(&mut self, name: &str) {
        if name.chars().count() > 64
            || name.chars().any(char::is_control)
            || name.eq_ignore_ascii_case("ungrouped")
        {
            self.notice = Some(
                "Use up to 64 characters on one line. Leave the group blank for Ungrouped.".into(),
            );
            return;
        }
        let canonical = self
            .folders
            .get(&name.to_lowercase())
            .map_or(name, String::as_str)
            .to_owned();
        let apps: Vec<_> = self
            .apps
            .iter()
            .filter(|app| self.selection.contains(&app.id.key))
            .map(|app| app.id.clone())
            .collect();
        if apps.is_empty() && canonical.is_empty() {
            self.notice = Some("Enter a name for the folder.".into());
            return;
        }
        match self.store.save_groups(&apps, &canonical) {
            Ok(()) => {
                self.refresh_apps();
                if self
                    .folder
                    .as_ref()
                    .is_some_and(|folder| *folder != canonical.to_lowercase())
                    && !apps.is_empty()
                {
                    self.selection.clear();
                    self.selection_anchor = None;
                    self.update_editor();
                } else {
                    self.group_input = TextEdit::new(&canonical);
                }
                self.notice = Some(if apps.is_empty() {
                    format!("Folder {canonical} is ready.")
                } else if canonical.is_empty() {
                    format!("Moved {} to Ungrouped.", app_count(apps.len()))
                } else {
                    format!("Moved {} to {canonical}.", app_count(apps.len()))
                });
            }
            Err(error) => self.notice = Some(format!("Could not save the group. {error}")),
        }
    }

    fn save_alias(&mut self) {
        let Some(app) = &self.selected else { return };
        let name = self.alias.text.trim();
        if name.chars().count() > 64 || name.chars().any(char::is_control) {
            self.notice = Some("Use up to 64 characters on one line.".into());
            return;
        }
        match self.store.save_alias(app, name) {
            Ok(()) => {
                self.notice = Some(
                    if name.is_empty() {
                        "Original name restored."
                    } else {
                        "Alias saved."
                    }
                    .into(),
                );
                self.refresh();
                self.refresh_apps();
            }
            Err(error) => self.notice = Some(format!("Could not save the alias. {error}")),
        }
    }

    fn icon(&self, path: &str) -> Option<Image> {
        let cache = self.icons.as_ref()?;
        Some(
            self.app_icons
                .borrow_mut()
                .entry(path.to_owned())
                .or_insert_with(|| cache.app_icon(path, 18.0))
                .clone(),
        )
    }
}

fn app_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "app" } else { "apps" })
}

fn poll(state: &mut State, clock: f64, tx: &mut Tx) -> bool {
    if !state.live {
        return false;
    }

    if clock < state.next_sample {
        tx.wake_after(Duration::from_secs_f64(state.next_sample - clock));
        return false;
    }
    state.next_sample = clock + 1.0;
    tx.wake_after(Duration::from_secs(1));
    let now = Local::now();
    let today = now.date_naive();
    let previous_today = state.today;
    let mut refresh = today != previous_today;
    if state.day == state.today {
        state.day = today;
    }
    if state.report_date == state.today {
        state.report_date = today;
    }
    state.today = today;

    let mut current = Sample {
        app: if state.paused {
            None
        } else {
            platform::foreground()
        },
        time: Utc::now().timestamp_millis(),
    };
    let domain = state.website_probe.as_ref().and_then(|probe| {
        let domain = probe.result(state.sample.as_ref().map(|sample| sample.time));
        if let Some(target) = current.app.as_mut().and_then(|app| app.browser.take()) {
            probe.sample(current.time, target);
        }
        domain
    });
    let old = state.sample.replace(current);
    let current = state.sample.as_ref().unwrap();
    let mut next_session = None;
    if let Some(previous) = old
        && let Some(focused) = previous.app
        && (1..=2500).contains(&(current.time - previous.time))
    {
        match state.store.record(
            &focused.app,
            previous.time,
            current.time,
            state.continuing,
            focused.page_title.as_deref(),
            domain.as_deref(),
        ) {
            Ok(id) => {
                if current
                    .app
                    .as_ref()
                    .is_some_and(|next| next.app.key == focused.app.key)
                {
                    next_session = Some(id);
                }
                state.tracking_error = None;
                refresh = true;
            }
            Err(error) => {
                state.tracking_error = Some(format!("Could not record activity. {error}"))
            }
        }
    }
    state.continuing = next_session;

    if state.screen == Screen::Settings && clock >= state.next_scan {
        state.open_apps = platform::open_apps();
        state.next_scan = clock + 3.0;
        state.refresh_apps();
    }
    if refresh {
        state.refresh();
        if state.screen == Screen::Calendar {
            state.refresh_calendar_day(previous_today);
            if today != previous_today {
                state.refresh_calendar_day(today);
            }
        }
    }
    true
}

fn preview() -> State {
    State::new(
        Store::open(Path::new(":memory:")).expect("open preview database"),
        false,
    )
}

fn sample_data(state: &mut State) {
    use chrono::TimeZone;
    let apps = [
        ("code", "Code", "VS Code"),
        ("chrome", "chrome", "Chrome"),
        ("slack", "slack", "Slack"),
        ("explorer", "explorer", "File Explorer"),
        ("firefox", "firefox", "Firefox"),
    ];
    let midnight = Local
        .from_local_datetime(&state.day.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    for (index, start, length) in [
        (0, 540, 64),
        (1, 604, 37),
        (2, 641, 22),
        (0, 677, 53),
        (3, 755, 16),
        (1, 784, 48),
        (0, 842, 35),
        (4, 885, 12),
    ] {
        let (key, name, alias) = apps[index];
        let app = AppId {
            key: key.into(),
            name: name.into(),
        };
        state
            .store
            .record(
                &app,
                midnight + start * 60_000,
                midnight + (start + length) * 60_000,
                None,
                match index {
                    1 if start < 700 => Some("Chrome Developers"),
                    1 => Some("Rust documentation"),
                    4 => Some("MDN Web Docs"),
                    _ => None,
                },
                match index {
                    1 if start < 700 => Some("chrome.com"),
                    1 => Some("rust-lang.org"),
                    4 => Some("mozilla.org"),
                    _ => None,
                },
            )
            .unwrap();
        state.store.save_alias(&app, alias).unwrap();
    }
    let yesterday = state.day.checked_sub_days(Days::new(1)).unwrap();
    let start = Local
        .from_local_datetime(&yesterday.and_hms_opt(10, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    let editor = AppId {
        key: "code".into(),
        name: "Code".into(),
    };
    state
        .store
        .record(&editor, start, start + 117 * 60_000, None, None, None)
        .unwrap();
    state.refresh();
}

fn spec() -> AppSpec<State> {
    AppSpec {
        min_size: Some((390.0, 480.0)),
        fonts: &[
            include_bytes!("../assets/fonts/MonaSans.ttf"),
            include_bytes!("../assets/fonts/GeistMono.ttf"),
        ],
        families: ("Mona Sans", "Geist Mono"),
        setup: Some(|state, waker| {
            if state.live {
                state.icons = Some(ImageCache::new(waker));
            }
        }),
        poll: Some(poll),
        scenes: &[
            ("populated", sample_data),
            ("aliases", |s| {
                sample_data(s);
                s.show_screen(Screen::Settings);
                s.select(&s.apps[0].id.key.clone(), false, false);
            }),
            ("calendar", |s| {
                sample_data(s);
                s.show_screen(Screen::Calendar);
            }),
            ("reports", |s| {
                sample_data(s);
                for (name, group) in [
                    ("code", "Work"),
                    ("chrome", "Work"),
                    ("slack", "Communication"),
                    ("firefox", "Reading"),
                ] {
                    let app = AppId {
                        key: name.into(),
                        name: name.into(),
                    };
                    s.store.save_groups(&[app], group).unwrap();
                }
                s.show_screen(Screen::Reports);
            }),
        ],
        ..AppSpec::new("Molo Tracker", (1120.0, 820.0), preview, ui::view)
    }
}

fn tray_icon() -> TrayIcon {
    let mut pixels = Vec::new();
    for y in 0..32 {
        for x in 0..32 {
            let bar = ((6..11).contains(&x) && (16..24).contains(&y))
                || ((13..18).contains(&x) && (8..24).contains(&y))
                || ((20..25).contains(&x) && (12..24).contains(&y));
            pixels.extend(if bar {
                [218, 240, 158, 255]
            } else {
                [28, 61, 48, 255]
            });
        }
    }
    TrayIcon::rgba(32, 32, pixels)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if rune::tooling::is_headless() {
        App::new(spec()).try_run()?;
        return Ok(());
    }
    let _instance = platform::Instance::acquire()?;
    rune::set_app_icon_resource(1);
    let dir = PathBuf::from(
        std::env::var_os("LOCALAPPDATA")
            .ok_or("Windows did not provide a local app data folder.")?,
    )
    .join("Molo Tracker");
    std::fs::create_dir_all(&dir)?;
    let store = Store::open(&dir.join("activity.sqlite3"))?;
    let app = App::new(spec());
    let handle = app.handle();
    let tray = Tray::new(
        TraySpec::new("molo-tracker", "Molo Tracker", tray_icon())
            .tooltip("Molo Tracker")
            .menu(vec![
                TrayItem::action("open", "Open activity"),
                TrayItem::action("quit", "Quit Molo"),
            ]),
        move |event| match event {
            TrayEvent::Activate { .. } => handle.open(),
            TrayEvent::Menu(id) if id == "open" => handle.open(),
            TrayEvent::Menu(id) if id == "quit" => handle.quit(),
            _ => {}
        },
    );
    let tray_available = tray.is_ok();
    let app = app.init(move || {
        let mut state = State::new(store, true);
        if !tray_available {
            state.notice = Some(
                "The notification area is unavailable. Closing Molo will stop tracking.".into(),
            );
        }
        state
    });
    let app = if tray_available {
        app.keep_running()
    } else {
        app
    };
    app.try_run()?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        if rune::tooling::is_headless() {
            eprintln!("{error}");
        } else {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
            let title: Vec<_> = "Molo Tracker".encode_utf16().chain([0]).collect();
            let message: Vec<_> = error.to_string().encode_utf16().chain([0]).collect();
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    message.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONERROR,
                )
            };
        }
        std::process::exit(1);
    }
}
