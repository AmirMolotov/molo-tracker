#![windows_subsystem = "windows"]

mod platform;
mod report;
mod store;
mod sync;
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
use store::{Application, BrowserPage, DEFAULT_IDLE_SECONDS, Device, Session, Store, Website};
use websites::WebsiteProbe;

struct Sample {
    app: Option<FocusedApp>,
    time: i64,
    idle_at: Option<i64>,
    idle_exempt: bool,
}

impl Sample {
    fn active_end(&self, current: &Self, last_input: Option<i64>) -> Option<i64> {
        let now = current.time;
        // Input before the previous deadline extends the active interval without a gap.
        let end = match (self.idle_exempt, self.idle_at) {
            (true, _) => now,
            (false, Some(deadline))
                if !last_input.is_some_and(|input| self.time < input && input <= deadline) =>
            {
                now.min(deadline)
            }
            _ => now.min(current.idle_at.unwrap_or(now)),
        };
        ((1..=2500).contains(&(now - self.time)) && end > self.time).then_some(end)
    }
}

#[cfg(test)]
#[test]
fn idle_deadline_clips_the_last_recorded_interval() {
    let previous = Sample {
        app: None,
        time: 1000,
        idle_at: Some(1400),
        idle_exempt: false,
    };
    let current = Sample {
        app: None,
        time: 2050,
        idle_at: Some(1400),
        idle_exempt: true,
    };
    assert_eq!(previous.active_end(&current, Some(400)), Some(1400));
    let exempt = Sample {
        idle_exempt: true,
        ..previous
    };
    let regular = Sample {
        idle_exempt: false,
        ..current
    };
    assert_eq!(exempt.active_end(&regular, Some(400)), Some(2050));
    let previous = Sample {
        idle_exempt: false,
        ..exempt
    };
    let resumed = Sample {
        idle_at: Some(2900),
        ..regular
    };
    assert_eq!(previous.active_end(&resumed, Some(1900)), Some(1400));
    let extended = Sample {
        idle_at: Some(2300),
        ..resumed
    };
    assert_eq!(previous.active_end(&extended, Some(1300)), Some(2050));
    let gap = Sample {
        time: 5000,
        ..extended
    };
    assert_eq!(previous.active_end(&gap, Some(1300)), None);
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

#[derive(Clone, Copy)]
enum GroupEdit {
    Rename,
    Delete,
}

struct State {
    store: Store,
    sync: Option<sync::Client>,
    sync_error: Option<String>,
    live: bool,
    start_with_windows: bool,
    startup_error: Option<String>,
    paused: bool,
    idle: bool,
    idle_seconds: u32,
    idle_input: TextEdit,
    idle_notice: Option<String>,
    idle_exceptions: BTreeSet<String>,
    idle_exceptions_open: bool,
    idle_exception_search: TextEdit,
    idle_exception_error: Option<String>,
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
    chart_search: TextEdit,
    timetable_search: TextEdit,
    timetable_titles: Option<BTreeMap<i64, Vec<String>>>,
    prior_total: i64,
    month: NaiveDate,
    month_totals: [i64; 31],
    expanded: Option<i64>,
    pages: Vec<BrowserPage>,
    apps: Vec<Application>,
    folder_search: TextEdit,
    screen: Screen,
    cutoff: u32,
    cutoff_input: TextEdit,
    cutoff_notice: Option<String>,
    selected: Option<AppId>,
    selection: BTreeSet<String>,
    selection_anchor: Option<String>,
    folders: BTreeMap<String, String>,
    folder: Option<String>,
    folder_name: TextEdit,
    folder_notice: Option<String>,
    group_edit: Option<GroupEdit>,
    drag_preview: Option<DragPreview>,
    alias: TextEdit,
    group_input: TextEdit,
    report_period: Period,
    report_date: NaiveDate,
    report: Report,
    report_device: Option<String>,
    report_devices: Vec<Device>,
    report_group: Option<String>,
    report_websites: bool,
    report_app_search: TextEdit,
    report_website_search: TextEdit,
    settings_websites: bool,
    websites: Vec<Website>,
    website_search: TextEdit,
    website_folder: Option<String>,
    website_group_domain: Option<String>,
    website_group_input: TextEdit,
    website_notice: Option<String>,
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
        let (idle_seconds, idle_notice) = match store.idle_seconds() {
            Ok(seconds) => (seconds, None),
            Err(error) => (
                DEFAULT_IDLE_SECONDS,
                Some(format!("Could not read the idle timeout. {error}")),
            ),
        };
        let (idle_exceptions, idle_exception_error) = match store.idle_exceptions() {
            Ok(exceptions) => (exceptions, None),
            Err(error) => (
                BTreeSet::new(),
                Some(format!("Could not read idle exceptions. {error}")),
            ),
        };
        let mut state = Self {
            store,
            sync: None,
            sync_error: None,
            live,
            start_with_windows: false,
            startup_error: None,
            today,
            day: today,
            first: today,
            paused: false,
            idle: false,
            idle_seconds,
            idle_input: TextEdit::new(idle_seconds.to_string()),
            idle_notice,
            idle_exceptions,
            idle_exceptions_open: false,
            idle_exception_search: TextEdit::new(""),
            idle_exception_error,
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
            chart_search: TextEdit::new(""),
            timetable_search: TextEdit::new(""),
            timetable_titles: None,
            prior_total: 0,
            month: today.with_day(1).unwrap(),
            month_totals: [0; 31],
            expanded: None,
            pages: Vec::new(),
            apps: Vec::new(),
            folder_search: TextEdit::new(""),
            screen: Screen::Activity,
            cutoff,
            cutoff_input: TextEdit::new(cutoff.to_string()),
            cutoff_notice,
            selected: None,
            selection: BTreeSet::new(),
            selection_anchor: None,
            folders: BTreeMap::new(),
            folder: None,
            folder_name: TextEdit::new(""),
            folder_notice: None,
            group_edit: None,
            drag_preview: None,
            alias: TextEdit::new(""),
            group_input: TextEdit::new(""),
            report_period: Period::Daily,
            report_date: today,
            report: Report::default(),
            report_device: None,
            report_devices: Vec::new(),
            report_group: None,
            report_websites: false,
            report_app_search: TextEdit::new(""),
            report_website_search: TextEdit::new(""),
            settings_websites: false,
            websites: Vec::new(),
            website_search: TextEdit::new(""),
            website_folder: None,
            website_group_domain: None,
            website_group_input: TextEdit::new(""),
            website_notice: None,
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
        self.timetable_titles = None;
        if self.screen == Screen::Activity && !self.timetable_search.text.trim().is_empty() {
            self.load_timetable_titles();
        }
        if self.screen == Screen::Reports {
            self.refresh_report();
        }
    }

    fn login_sync(&mut self) {
        if self.live
            && let Some(sync) = &mut self.sync
        {
            sync.login(&self.store);
        }
    }

    fn logout_sync(&mut self) {
        if self
            .sync
            .as_mut()
            .is_some_and(|sync| sync.logout(&mut self.store))
        {
            self.refresh();
            self.refresh_apps();
            if self.settings_websites {
                self.refresh_websites();
            }
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

    fn load_timetable_titles(&mut self) {
        match self.store.page_titles(self.day) {
            Ok(pages) => {
                let mut titles = BTreeMap::<_, Vec<_>>::new();
                for (id, title) in pages {
                    titles.entry(id).or_default().push(title);
                }
                self.timetable_titles = Some(titles);
            }
            Err(error) => {
                self.notice = Some(format!("Could not search browser page titles. {error}"))
            }
        }
    }

    fn edit_timetable_search(&mut self) -> &mut TextEdit {
        if self.timetable_titles.is_none() {
            self.load_timetable_titles();
        }
        &mut self.timetable_search
    }

    fn select_day(&mut self, day: NaiveDate) {
        self.day = day;
        self.expanded = None;
        self.refresh();
    }

    fn show_screen(&mut self, screen: Screen) {
        if self.screen == Screen::Settings
            && screen != Screen::Settings
            && let Some(sync) = &mut self.sync
        {
            sync.clear_password();
        }
        self.drag_preview = None;
        self.screen = screen;
        self.notice = None;
        match screen {
            Screen::Activity => self.refresh(),
            Screen::Calendar => self.show_month(self.day.with_day(1).unwrap()),
            Screen::Reports => self.refresh_report(),
            Screen::Settings => {
                if self.live {
                    self.open_apps = platform::open_apps();
                    match platform::starts_with_windows() {
                        Ok(enabled) => {
                            self.start_with_windows = enabled;
                            self.startup_error = None;
                        }
                        Err(error) => {
                            self.startup_error = Some(format!(
                                "Could not read the Windows startup setting. {error}"
                            ));
                        }
                    }
                }
                self.refresh_apps();
                if self.settings_websites {
                    self.refresh_websites();
                }
            }
        }
    }

    fn refresh_report(&mut self) {
        self.load_report(self.report_device.clone());
    }

    fn load_report(&mut self, device: Option<String>) {
        let result = self.store.report_devices().and_then(|devices| {
            let device = device.filter(|id| devices.iter().any(|device| &device.id == id));
            let report = Report::load(
                &self.store,
                self.report_period,
                self.report_date,
                device.as_deref(),
            )?;
            Ok((report, devices, device))
        });
        match result {
            Ok((report, devices, device)) => {
                if self.report_device != device {
                    self.report_group = None;
                }
                self.report = report;
                self.report_devices = devices;
                self.report_device = device;
            }
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

    fn refresh_websites(&mut self) {
        match self.store.websites() {
            Ok(websites) => {
                if self
                    .website_group_domain
                    .as_ref()
                    .is_some_and(|domain| !websites.iter().any(|website| &website.domain == domain))
                {
                    self.website_group_domain = None;
                }
                self.websites = websites;
            }
            Err(error) => self.website_notice = Some(format!("Could not read websites. {error}")),
        }
    }

    fn select_website(&mut self, domain: &str) {
        if let Some(site) = self.websites.iter().find(|site| site.domain == domain) {
            self.website_group_domain = Some(domain.to_owned());
            self.website_group_input = TextEdit::new(site.group.as_deref().unwrap_or(""));
            self.website_notice = None;
        }
    }

    fn note_website_activity(&mut self, domain: &str, time: i64) {
        if !self.websites.iter().any(|site| site.domain == domain) {
            self.refresh_websites();
            return;
        }
        let parent = websites::parent_domain(domain);
        let mut changed = false;
        for site in &mut self.websites {
            if (site.domain == domain || Some(site.domain.as_str()) == parent)
                && site.last_seen < time
            {
                site.last_seen = time;
                changed = true;
            }
        }
        if changed {
            Website::sort_recent(&mut self.websites);
        }
    }

    fn save_website_group(&mut self) {
        let name = self.website_group_input.text.trim().to_owned();
        if name.is_empty() || !valid_folder_name(&name) {
            self.website_notice = Some("Enter a group name of up to 64 characters.".into());
            return;
        }
        self.update_website_group(Some(&name));
    }

    fn update_website_group(&mut self, name: Option<&str>) {
        let Some(domain) = self.website_group_domain.clone() else {
            return;
        };
        let name = if name == Some("") && websites::parent_domain(&domain).is_none() {
            None
        } else {
            name
        };
        match self.store.save_website_group(&domain, name) {
            Ok(()) => {
                self.website_notice = None;
                self.refresh_apps();
                self.refresh_websites();
                if let Some(site) = self.websites.iter().find(|site| site.domain == domain) {
                    self.website_group_input = TextEdit::new(site.group.as_deref().unwrap_or(""));
                }
            }
            Err(error) => {
                self.website_notice = Some(format!("Could not save the website group. {error}"))
            }
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

    fn save_idle(&mut self) {
        let Ok(seconds) = self.idle_input.text.trim().parse::<u32>() else {
            self.idle_notice = Some("Enter a nonnegative whole number of seconds.".into());
            return;
        };
        if seconds.checked_mul(1000).is_none() {
            self.idle_notice = Some(format!("Enter at most {} seconds.", u32::MAX / 1000));
            return;
        }
        match self.store.save_idle_seconds(seconds) {
            Ok(()) => {
                if self.idle_seconds != seconds {
                    self.sample = None;
                    self.continuing = None;
                }
                self.idle_seconds = seconds;
                self.idle_notice = Some("Idle timeout saved.".into());
            }
            Err(error) => {
                self.idle_notice = Some(format!("Could not save the idle timeout. {error}"))
            }
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
                            last_seen: None,
                        });
                    }
                }
                apps.sort_by_cached_key(|app| {
                    (
                        std::cmp::Reverse(app.last_seen),
                        app.label().to_lowercase(),
                        app.id.key.clone(),
                    )
                });
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
        if self
            .website_folder
            .as_ref()
            .is_some_and(|key| !key.is_empty() && !self.folders.contains_key(key))
        {
            self.website_folder = None;
        }
    }

    fn toggle_start_with_windows(&mut self) {
        let enabled = !self.start_with_windows;
        if self.live
            && let Err(error) = platform::set_start_with_windows(enabled)
        {
            self.startup_error = Some(format!(
                "Could not change the Windows startup setting. {error}"
            ));
            return;
        }
        self.start_with_windows = enabled;
        self.startup_error = None;
    }

    fn toggle_idle_exception(&mut self, app: &AppId) {
        let enabled = !self.idle_exceptions.contains(&app.key);
        match self.store.set_idle_exception(app, enabled) {
            Ok(()) => match self.store.idle_exceptions() {
                Ok(exceptions) => {
                    self.idle_exceptions = exceptions;
                    self.idle_exception_error = None;
                }
                Err(error) => {
                    self.idle_exception_error =
                        Some(format!("Could not refresh idle exceptions. {error}"))
                }
            },
            Err(error) => {
                self.idle_exception_error =
                    Some(format!("Could not save the idle exception. {error}"))
            }
        }
    }

    fn refresh_synced_preferences(&mut self) {
        if let Ok(cutoff) = self.store.cutoff()
            && cutoff != self.cutoff
        {
            if self.cutoff_input.text == self.cutoff.to_string() {
                self.cutoff_input = TextEdit::new(cutoff.to_string());
            }
            self.cutoff = cutoff;
        }
        if let Ok(idle) = self.store.idle_seconds()
            && idle != self.idle_seconds
        {
            if self.idle_input.text == self.idle_seconds.to_string() {
                self.idle_input = TextEdit::new(idle.to_string());
            }
            self.idle_seconds = idle;
            self.sample = None;
            self.continuing = None;
        }
        if let Ok(exceptions) = self.store.idle_exceptions() {
            self.idle_exceptions = exceptions;
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.sample = None;
        self.continuing = None;
    }

    fn visible_apps(&self) -> impl Iterator<Item = &Application> {
        let search = self.folder_search.text.trim().to_lowercase();
        self.apps.iter().filter(move |app| {
            self.folder
                .as_ref()
                .is_none_or(|folder| report::group_key(app) == *folder)
                && app.matches(&search)
        })
    }

    fn open_folder(&mut self, folder: Option<String>) {
        self.folder_name = TextEdit::new(
            folder
                .as_ref()
                .and_then(|key| self.folders.get(key))
                .map_or("", String::as_str),
        );
        self.group_edit = None;
        self.folder_notice = None;
        self.folder = folder;
        self.selection.clear();
        self.selection_anchor = None;
        self.update_editor();
    }

    fn change_folder(&mut self, delete: bool) {
        let Some(key) = self
            .folder
            .clone()
            .filter(|key| self.folders.contains_key(key))
        else {
            return;
        };
        let name = self.folder_name.text.trim().to_owned();
        if !delete && (name.is_empty() || !valid_folder_name(&name)) {
            self.folder_notice =
                Some("Enter a group name of up to 64 characters. Ungrouped is reserved.".into());
            return;
        }
        match self
            .store
            .change_folder(&key, (!delete).then_some(name.as_str()))
        {
            Ok(true) => {
                self.refresh_apps();
                self.open_folder((!delete).then(|| name.to_lowercase()));
                if self.report_group.as_ref() == Some(&key) {
                    self.report_group = self.folder.clone();
                }
                self.folder_notice = Some(if delete {
                    "Group deleted. Its apps and websites were removed from it.".into()
                } else {
                    format!("Group renamed to {name}.")
                });
            }
            Ok(false) => self.folder_notice = Some("A group with that name already exists.".into()),
            Err(error) => self.folder_notice = Some(format!("Could not update the group. {error}")),
        }
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
        if !valid_folder_name(name) {
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
            self.notice = Some("Enter a name for the group.".into());
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
                    format!("Group {canonical} is ready.")
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

fn valid_folder_name(name: &str) -> bool {
    name.chars().count() <= 64
        && !name.chars().any(char::is_control)
        && !name.eq_ignore_ascii_case("ungrouped")
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

    if state
        .sync
        .as_mut()
        .is_some_and(|sync| sync.tick(&mut state.store, clock))
    {
        refresh = true;
        state.refresh_synced_preferences();
        match state.screen {
            Screen::Calendar => state.show_month(state.month),
            Screen::Settings => {
                state.refresh_apps();
                if state.settings_websites {
                    state.refresh_websites();
                }
                if state
                    .folder
                    .as_ref()
                    .is_some_and(|key| !key.is_empty() && !state.folders.contains_key(key))
                {
                    state.open_folder(None);
                }
            }
            _ => {}
        }
    }

    let last_input = (state.idle_seconds != 0).then(platform::idle_ms).flatten();
    let time = Utc::now().timestamp_millis();
    let last_input = last_input.map(|idle| time - i64::from(idle));
    let idle_at = last_input.map(|time| time + i64::from(state.idle_seconds) * 1000);
    let timed_out = idle_at.is_some_and(|deadline| time >= deadline);
    let mut idle_exempt = false;
    let app = if state.paused || (timed_out && state.idle_exceptions.is_empty()) {
        None
    } else {
        platform::foreground(|app| {
            idle_exempt = state.idle_exceptions.contains(&app.key)
                || state
                    .idle_exceptions
                    .contains(&format!("sync:{}", app.name.to_lowercase()));
            !timed_out || idle_exempt
        })
    };
    state.idle = timed_out && !idle_exempt;
    let mut current = Sample {
        app,
        time,
        idle_at,
        idle_exempt,
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
        && let Some(end) = previous.active_end(current, last_input)
        && let Some(focused) = previous.app
    {
        match state.store.record(
            &focused.app,
            previous.time,
            end,
            state.continuing,
            focused.page_title.as_deref(),
            domain.as_deref(),
        ) {
            Ok(id) => {
                if end == current.time
                    && current
                        .app
                        .as_ref()
                        .is_some_and(|next| next.app.key == focused.app.key)
                {
                    next_session = Some(id);
                }
                state.tracking_error = None;
                if state.screen == Screen::Settings
                    && state.settings_websites
                    && state.drag_preview.is_none()
                    && let Some(domain) = domain.as_deref()
                {
                    state.note_website_activity(domain, end);
                }
                refresh = true;
            }
            Err(error) => {
                state.tracking_error = Some(format!("Could not record activity. {error}"))
            }
        }
    }
    state.continuing = next_session;

    if state.screen == Screen::Settings && state.drag_preview.is_none() && clock >= state.next_scan
    {
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
                    1 if start < 700 => Some("developer.chrome.com"),
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
        on_key: Some(ui::search_key),
        scenes: &[
            ("populated", sample_data),
            ("synced", |s| {
                sample_data(s);
                let start = s.sessions.last().unwrap().start + 15 * 60_000;
                let device = "sample-laptop".to_owned();
                let devices = BTreeMap::from([(device.clone(), "Laptop".to_owned())]);
                s.store
                    .apply_sync(
                        &[],
                        1,
                        &[sync::Delta {
                            device,
                            id: 1,
                            app: "Firefox".to_owned(),
                            start,
                            end: start + 30 * 60_000,
                            pages: Vec::new(),
                            domains: Vec::new(),
                        }],
                        &devices,
                        None,
                    )
                    .unwrap();
                s.refresh();
            }),
            ("settings", |s| {
                sample_data(s);
                s.sync = Some(sync::Client::preview());
                s.show_screen(Screen::Settings);
            }),
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
    let mut store = Store::open(&dir.join("activity.sqlite3"))?;
    let (sync, sync_error) = match sync::Client::open(&dir.join("sync.toml"), &mut store) {
        Ok(client) => (Some(client), None),
        Err(error) => (None, Some(format!("Sync setup failed. {error}"))),
    };
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
        state.sync = sync;
        state.sync_error = sync_error;
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
