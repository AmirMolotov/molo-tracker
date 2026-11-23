use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use chrono::{Datelike, Days, Local, TimeZone};
use rune::tok::motion;
use rune::{
    Color, Cx, El, ScrollSpec, TextEdit, TextStyle, Transition, Tx, col, el, row, text, text_input,
};

use crate::store::Session;
use crate::{Screen, State};

mod reports;

const BASE: u32 = 0xe9ebe5;
const SURFACE: u32 = 0xf7f8f3;
const LINE: u32 = 0xd4d9ce;
const INK: u32 = 0x192920;
const MUTED: u32 = 0x4e6155;
const FOREST: u32 = 0x193d30;
const LIME: u32 = 0xdcf5a0;
const PALE: u32 = 0xbfd1c1;
const PAPER: u32 = 0xf3f7ec;

const BODY: TextStyle = TextStyle::new(14.0, 450.0);
const LABEL: TextStyle = TextStyle::new(11.0, 620.0).mono().tracking(0.7);
const TIME: TextStyle = TextStyle::new(13.0, 500.0).mono();
const TITLE: TextStyle = TextStyle::new(27.0, 560.0).tracking(-0.9);

fn color(rgb: u32) -> Color {
    let channel = |shift: u32| rune::theme::srgb_decode(((rgb >> shift) & 255) as f32 / 255.0);
    Color {
        r: channel(16),
        g: channel(8),
        b: channel(0),
        a: 1.0,
    }
}

fn label(value: impl Into<String>, style: TextStyle, color: Color) -> El {
    text(value).font(style).color(color)
}

fn heading(value: &str, level: u32) -> El {
    let style = if level == 1 {
        TextStyle::new(38.0, 550.0).tracking(-1.5)
    } else {
        TITLE
    };
    label(value, style, color(INK)).role("heading").level(level)
}

fn panel() -> El {
    col()
        .min_w(0.0)
        .padding(22.0)
        .gap(14.0)
        .radius(15.0)
        .fill(color(SURFACE))
        .border(1.0, color(LINE))
}

fn button(
    key: &str,
    name: &str,
    selected: bool,
    disabled: bool,
    f: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    let mut node = row()
        .key(key)
        .role("button")
        .label(name)
        .disabled(disabled)
        .focusable(!disabled)
        .focus_ring(2.0, 2.0, color(FOREST))
        .center()
        .h(44.0)
        .px(13.0)
        .radius(9.0)
        .border(1.0, if selected { color(FOREST) } else { color(LINE) })
        .fill(if selected {
            color(FOREST)
        } else {
            color(SURFACE)
        })
        .child(label(
            name,
            BODY,
            if selected { color(PAPER) } else { color(MUTED) },
        ));
    if !disabled {
        node = node
            .hover(if selected {
                color(0x2b5543)
            } else {
                color(0xe0e9da)
            })
            .motion(motion::SNAPPY)
            .on_press_tx(f);
    } else {
        node = node.opacity(0.55);
    }
    node
}

fn separator() -> El {
    el().w_full().h(1.0).fill(color(LINE))
}

fn search_field(
    key: &str,
    hint: &str,
    edit: &TextEdit,
    access: fn(&mut State) -> &mut TextEdit,
) -> El {
    let gold = color(0xdabb84);
    let field = key.to_owned();
    let mut bar = row()
        .w_full()
        .min_w(0.0)
        .h(44.0)
        .radius(7.0)
        .center_y()
        .gap(10.0)
        .fill(color(0x252c28))
        .border(1.0, color(0x54594a))
        .focus_within_ring(2.0, 1.0, gold)
        .child(
            el().size(34.0, 34.0)
                .ml(4.0)
                .radius(4.0)
                .fill(color(0x383f36))
                .center()
                .child(
                    el().size(17.0, 17.0).a11y_hidden().shape(
                        rune::Shape::new()
                            .stroke(
                                (0..=24)
                                    .map(|i| {
                                        let angle = std::f32::consts::TAU * i as f32 / 24.0;
                                        [7.0 + 4.4 * angle.cos(), 7.0 + 4.4 * angle.sin()]
                                    })
                                    .collect::<Vec<_>>(),
                                1.6,
                                gold,
                            )
                            .line([10.4, 10.4], [15.0, 15.0], 1.6, rune::LineCap::Round, gold)
                            .view_box(17.0, 17.0),
                    ),
                ),
        )
        .child(
            text_input(edit, BODY)
                .key(key)
                .label(hint)
                .min_w(0.0)
                .grow(1.0)
                .color(color(PAPER))
                .placeholder(hint, color(0xbfc6b9))
                .caret(gold, color(0x58614d))
                .on_edit(access),
        );
    if !edit.text.is_empty() {
        bar = bar.child(
            row()
                .key(format!("{key}-clear"))
                .size(44.0, 44.0)
                .center()
                .role("button")
                .label("Clear search")
                .focusable(true)
                .focus_ring(2.0, -3.0, gold)
                .radius(6.0)
                .hover(color(0x464e42))
                .on_press_tx(move |s: &mut State, tx| {
                    *access(s) = TextEdit::new("");
                    tx.focus(&field, true);
                })
                .child(label("×", TextStyle::new(19.0, 500.0), gold)),
        );
    } else {
        bar = bar.child(el().w(11.0));
    }
    bar
}

fn search_query(edit: &TextEdit) -> String {
    edit.text.trim().to_lowercase()
}

fn empty_message(message: &str) -> El {
    label(message, BODY, color(MUTED)).wrap()
}

fn app_color(key: &str) -> Color {
    let colors = [0x315f49, 0x606f32, 0x3f7078, 0x926344, 0x73594d, 0x546b52];
    let mut hash = DefaultHasher::new();
    key.hash(&mut hash);
    color(colors[hash.finish() as usize % colors.len()])
}

fn app_mark(state: &State, key: &str) -> El {
    let mark = el().size(18.0, 18.0).center();
    if let Some(icon) = state.icon(key)
        && icon.size().is_some()
    {
        return mark.pixels(icon.pixels());
    }
    mark.child(el().size(8.0, 8.0).radius(4.0).fill(app_color(key)))
}

fn duration(ms: i64) -> String {
    let sec = ms / 1000;
    match sec {
        0 if ms > 0 => "<1s".into(),
        0..60 => format!("{sec}s"),
        60..3600 => format!("{}m {:02}s", sec / 60, sec % 60),
        _ => format!("{}h {:02}m", sec / 3600, sec / 60 % 60),
    }
}

fn time(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "—".into())
}

fn activity(s: &State, compact: bool) -> El {
    let mut by_app = BTreeMap::new();
    for session in &s.sessions {
        let total = by_app
            .entry(session.app.id.key.as_str())
            .or_insert((&session.app, 0));
        total.1 += session.end - session.start;
    }
    let mut apps: Vec<_> = by_app.into_iter().collect();
    apps.sort_by(|a, b| {
        b.1.1
            .cmp(&a.1.1)
            .then_with(|| a.1.0.label().cmp(b.1.0.label()))
    });
    let total = apps.iter().map(|(_, (_, ms))| ms).sum();
    let maximum = apps.first().map_or(1, |(_, (_, ms))| *ms);

    let date = s.day.format("%A, %b %-d, %Y").to_string();
    let date_controls = row()
        .gap(6.0)
        .center_y()
        .child(button(
            "previous",
            "Previous",
            false,
            s.day <= s.first,
            |s, _| {
                if let Some(day) = s.day.checked_sub_days(Days::new(1)) {
                    s.select_day(day);
                }
            },
        ))
        .child(button(
            "today",
            "Today",
            s.day == s.today,
            s.day == s.today,
            |s, _| {
                s.select_day(s.today);
            },
        ))
        .child(button("next", "Next", false, s.day >= s.today, |s, _| {
            if let Some(day) = s.day.checked_add_days(Days::new(1)) {
                s.select_day(day);
            }
        }));
    let day_row = if compact {
        col().gap(8.0)
    } else {
        row().center_y().gap(12.0)
    };
    let day_row = day_row
        .child(
            col()
                .min_w(0.0)
                .grow(1.0)
                .child(heading("Activity", 1))
                .child(label(date, BODY, color(MUTED)).wrap()),
        )
        .child(date_controls);

    let focus = col()
        .key("focus-summary")
        .min_w(0.0)
        .grow(1.0)
        .padding(24.0)
        .gap(13.0)
        .radius(15.0)
        .fill(color(FOREST))
        .child(label("FOCUS TIME", LABEL, color(PALE)))
        .child(label(
            duration(total),
            TextStyle::new(if compact { 48.0 } else { 55.0 }, 530.0).tracking(-3.0),
            color(LIME),
        ))
        .child(label(
            format!("{} apps  /  {} sessions", apps.len(), s.sessions.len()),
            BODY,
            color(PAPER),
        ))
        .child(el().min_h(12.0).grow(1.0))
        .child(el().w_full().h(1.0).fill(color(0x477056)))
        .child(
            label(
                comparison(total, s.prior_total, s.day == s.today),
                BODY,
                color(PALE),
            )
            .wrap(),
        );

    let query = search_query(&s.chart_search);
    let matched = apps.into_iter().filter(|(_, (app, _))| app.matches(&query));
    let has_match = matched.clone().next().is_some();
    let mut bars = col().key("bar-list").w_full().gap(11.0);
    for (key, (app, ms)) in matched {
        let ratio = (ms as f64 / maximum as f64) as f32;
        bars = bars.child(
            col()
                .key(format!("bar:{key}"))
                .w_full()
                .gap(5.0)
                .child(
                    row()
                        .w_full()
                        .gap(10.0)
                        .center_y()
                        .child(app_mark(s, key))
                        .child(
                            label(app.label(), BODY, color(INK))
                                .min_w(0.0)
                                .grow(1.0)
                                .truncate(),
                        )
                        .child(label(duration(ms), TIME, color(MUTED))),
                )
                .child(
                    el().w_full().h(6.0).radius(3.0).fill(color(BASE)).child(
                        el().key("value")
                            .w_frac(ratio)
                            .h(6.0)
                            .radius(3.0)
                            .fill(app_color(key))
                            .motion(motion::SNAPPY),
                    ),
                ),
        );
    }
    let chart = panel()
        .key("usage-chart")
        .grow(1.45)
        .child(heading("Usage by app", 2))
        .child(search_field(
            "chart-search",
            "Search apps",
            &s.chart_search,
            |s| &mut s.chart_search,
        ))
        .child(if s.sessions.is_empty() {
            col().h(118.0).gap(12.0).child(separator()).child(
                label(
                    "App totals will appear here after a focus session.",
                    BODY,
                    color(MUTED),
                )
                .wrap(),
            )
        } else if !has_match {
            col().h(118.0).child(empty_message("No matching apps."))
        } else {
            col()
                .key("chart-scroll")
                .w_full()
                .h(168.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(bars)
        });
    let overview = if compact {
        col().w_full()
    } else {
        row().w_full()
    };
    let overview = overview.gap(14.0).child(focus).child(chart);

    col()
        .key("activity-content")
        .w_full()
        .gap(16.0)
        .child(day_row)
        .child(overview)
        .child(timetable(s, compact))
}

fn aliases(s: &State, compact: bool, cx: &Cx<State>) -> El {
    use std::rc::Rc;

    let bounds = cx.window_layout("page");
    let targets = Rc::new(
        std::iter::once("")
            .chain(s.folders.keys().map(String::as_str))
            .filter_map(|key| {
                cx.window_layout(&format!("folder:{key}"))
                    .map(|rect| (key.to_owned(), rect))
            })
            .collect::<Vec<_>>(),
    );

    let mut folders = row()
        .key("app-folders")
        .gap(6.0)
        .flex_wrap()
        .child(button(
            "folder-all",
            "All apps",
            s.folder.is_none(),
            false,
            |s, tx| {
                s.open_folder(None);
                tx.scroll_to_instant("folder-apps", 0.0, 0.0);
            },
        ))
        .child(folder_button(s, "", "Ungrouped"));
    for (key, name) in &s.folders {
        folders = folders.child(folder_button(s, key, name));
    }
    let mut entries = col().key("app-rows").w_full().gap(3.0);
    for app in s.visible_apps() {
        let key = app.id.key.clone();
        let drag_key = key.clone();
        let targets = targets.clone();
        let selected = s.selection.contains(&key);
        let lifted = selected && s.drag_preview.is_some();
        let name = app.label().to_owned();
        let group = app.group.as_ref().filter(|_| s.folder.is_none());
        entries = entries.child(
            row()
                .key(format!("app:{key}"))
                .role("button")
                .label(&name)
                .focusable(true)
                .focus_ring(2.0, 2.0, color(FOREST))
                .min_h(44.0)
                .px(10.0)
                .gap(9.0)
                .center_y()
                .radius(8.0)
                .fill(color(if selected { 0xddebd6 } else { SURFACE }))
                .hover(color(if selected { 0xcfe2c9 } else { BASE }))
                .motion(motion::SNAPPY)
                .offset(0.0, if lifted { -2.0 } else { 0.0 })
                .opacity(if lifted { 0.56 } else { 1.0 })
                .child(app_mark(s, &key))
                .on_press_tx(move |s: &mut State, tx| {
                    let mods = tx.mods();
                    s.select(&key, mods.ctrl, mods.shift);
                })
                .on_drag(move |s: &mut State, drag, tx| match drag.phase {
                    rune::Phase::Down => drag.data = 0.0,
                    rune::Phase::Move if drag.dx.hypot(drag.dy) >= 6.0 || drag.data != 0.0 => {
                        if drag.data == 0.0 {
                            drag.data = 1.0;
                            if !s.selection.contains(&drag_key) {
                                s.select(&drag_key, false, false);
                            }
                            tx.animated();
                        }
                        let point = drag.window_point();
                        let target = targets
                            .iter()
                            .find(|(_, rect)| {
                                contains(*rect, point)
                                    && bounds.is_some_and(|rect| contains(rect, point))
                            })
                            .map(|(group, _)| group.clone());
                        s.drag_preview = Some(crate::DragPreview {
                            source: drag_key.clone(),
                            point,
                            target,
                        });
                    }
                    rune::Phase::Up => {
                        if s.drag_preview.take().is_some() {
                            tx.animated();
                        }
                        let point = drag.window_point();
                        if drag.data != 0.0
                            && !drag.cancelled()
                            && bounds.is_some_and(|rect| contains(rect, point))
                            && let Some((key, _)) =
                                targets.iter().find(|(_, rect)| contains(*rect, point))
                        {
                            let name = s.folders.get(key).cloned().unwrap_or_default();
                            s.move_to_group(&name);
                        }
                    }
                    _ => {}
                })
                .child(
                    label(name, BODY, color(INK))
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .children(
                    group.map(|name| label(name, LABEL, color(MUTED)).max_w(110.0).truncate()),
                ),
        );
    }
    if s.visible_apps().next().is_none() {
        entries = entries.child(empty_message(
            if search_query(&s.folder_search).is_empty() {
                "No apps in this folder."
            } else {
                "No matching apps in this folder."
            },
        ));
    }
    let list = panel().key("app-list").grow(1.0)
        .child(heading("App aliases and groups", 2))
        .child(label("Ctrl-click to toggle apps or Shift-click to select a range. Drop selected apps on a folder.", BODY, color(MUTED)).wrap())
        .child(folders)
        .child(search_field("folder-search", "Search apps in this folder", &s.folder_search, |s| &mut s.folder_search))
        .child(col().key("folder-apps").w_full().h(if compact { 210.0 } else { 298.0 })
            .scroll(ScrollSpec { stick_bottom: false }).child(entries));

    let mut editor = panel()
        .key("alias-editor")
        .grow(1.0)
        .child(heading("App settings", 2));
    if let Some(app) = &s.selected {
        editor = editor
            .child(label(&app.name, BODY, color(MUTED)).wrap())
            .child(label("ALIAS", LABEL, color(MUTED)))
            .child(
                el().w_full()
                    .h(44.0)
                    .px(10.0)
                    .center_y()
                    .radius(8.0)
                    .border(1.0, color(LINE))
                    .fill(color(BASE))
                    .child(
                        text_input(&s.alias, BODY)
                            .key("alias-input")
                            .label("Application alias")
                            .color(color(INK))
                            .w_full()
                            .on_edit(|s: &mut State| &mut s.alias)
                            .on_submit(|s: &mut State, _| s.save_alias()),
                    ),
            )
            .child(button("save-alias", "Save alias", true, false, |s, _| {
                s.save_alias()
            }))
            .child(separator());
    } else if !s.selection.is_empty() {
        editor = editor.child(label(
            format!("{} selected", crate::app_count(s.selection.len())),
            BODY,
            color(MUTED),
        ));
    }
    let hidden = s.selection.len()
        - s.visible_apps()
            .filter(|app| s.selection.contains(&app.id.key))
            .count();
    if hidden > 0 {
        editor = editor.child(
            label(
                format!("{} selected outside this search", crate::app_count(hidden)),
                BODY,
                color(MUTED),
            )
            .wrap(),
        );
    }
    let action = if s.selection.is_empty() {
        "Create folder"
    } else {
        "Move to folder"
    };
    editor = editor
        .child(label("FOLDER NAME", LABEL, color(MUTED)))
        .child(
            el().w_full()
                .h(44.0)
                .px(10.0)
                .center_y()
                .radius(8.0)
                .border(1.0, color(LINE))
                .fill(color(BASE))
                .child(
                    text_input(&s.group_input, BODY)
                        .key("group-input")
                        .label("Folder name")
                        .color(color(INK))
                        .w_full()
                        .on_edit(|s: &mut State| &mut s.group_input)
                        .on_submit(|s: &mut State, _| s.save_group()),
                ),
        )
        .child(button("save-group", action, true, false, |s, _| {
            s.save_group()
        }));
    if let Some(message) = &s.notice {
        editor = editor.child(label(message, BODY, color(FOREST)).wrap());
    }
    let split = if compact { col() } else { row().items_start() };
    split
        .key("aliases-content")
        .w_full()
        .gap(14.0)
        .child(list)
        .child(editor)
}

fn contains([x, y, w, h]: [f32; 4], [px, py]: [f32; 2]) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

fn folder_button(s: &State, key: &str, name: &str) -> El {
    let selected = s.folder.as_deref() == Some(key);
    let over = s
        .drag_preview
        .as_ref()
        .and_then(|drag| drag.target.as_deref())
        == Some(key);
    let key = key.to_owned();
    let ink = color(if selected { PAPER } else { FOREST });
    row()
        .key(format!("folder:{key}"))
        .role("button")
        .label(name)
        .focusable(true)
        .focus_ring(2.0, 2.0, color(FOREST))
        .h(44.0)
        .max_w(230.0)
        .min_w(0.0)
        .px(10.0)
        .center_y()
        .gap(7.0)
        .radius(8.0)
        .fill(color(if over {
            0xc1daa9
        } else if selected {
            FOREST
        } else {
            BASE
        }))
        .hover(color(if selected { 0x2b5543 } else { 0xddebd6 }))
        .on_press_tx(move |s: &mut State, tx| {
            s.open_folder(Some(key.clone()));
            tx.scroll_to_instant("folder-apps", 0.0, 0.0);
        })
        .child(
            col()
                .size(15.0, 12.0)
                .child(el().w(6.0).h(3.0).radius(1.0).fill(ink))
                .child(el().w(15.0).h(9.0).radius(2.0).fill(ink)),
        )
        .child(
            label(name, BODY, ink)
                .min_w(0.0)
                .truncate()
                .help_when_truncated(name),
        )
}

fn drag_ghost(s: &State, viewport: [f32; 2]) -> Option<El> {
    let drag = s.drag_preview.as_ref()?;
    let app = s.apps.iter().find(|app| app.id.key == drag.source)?;
    let count = s.selection.len();
    let [width, height] = viewport;
    let card_width = 224.0;
    let card_height = 46.0;
    let [x, y] = drag.point;
    let left = if x + card_width + 22.0 > width {
        x - card_width - 16.0
    } else {
        x + 16.0
    };
    let top = if y + card_height + 22.0 > height {
        y - card_height - 16.0
    } else {
        y + 16.0
    };

    Some(
        row()
            .key("app-drag-ghost")
            .absolute(
                left.clamp(4.0, (width - card_width - 4.0).max(4.0)),
                top.clamp(4.0, (height - card_height - 4.0).max(4.0)),
            )
            .size(card_width, card_height)
            .px(11.0)
            .gap(8.0)
            .center_y()
            .radius(10.0)
            .fill(color(FOREST))
            .border(
                1.0,
                color(if drag.target.is_some() {
                    LIME
                } else {
                    0x52735d
                }),
            )
            .shadow(6.0, 14.0, color(INK).alpha(0.28))
            .inert()
            .a11y_hidden()
            .child(app_mark(s, &drag.source))
            .child(
                label(app.label(), BODY, color(PAPER))
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate(),
            )
            .children((count > 1).then(|| label(crate::app_count(count), LABEL, color(LIME)))),
    )
}

fn settings(s: &State, compact: bool, cx: &Cx<State>) -> El {
    let input = el()
        .w(95.0)
        .h(44.0)
        .px(10.0)
        .center_y()
        .radius(8.0)
        .border(1.0, color(LINE))
        .fill(color(BASE))
        .child(
            text_input(&s.cutoff_input, BODY)
                .key("cutoff-input")
                .label("Cutoff in seconds")
                .color(color(INK))
                .w_full()
                .on_edit(|s: &mut State| &mut s.cutoff_input)
                .on_submit(|s: &mut State, _| s.save_cutoff()),
        );
    let mut cutoff = panel().key("cutoff")
        .child(heading("Timetable cutoff", 2))
        .child(label("Show sessions that meet this minimum. Shorter sessions still count in totals. Enter 0 to show every session.", BODY, color(MUTED)).wrap())
        .child(row().center_y().gap(8.0)
            .child(input).child(label("seconds", BODY, color(MUTED)))
            .child(button("save-cutoff", "Save", true, false, |s, _| s.save_cutoff())));
    if let Some(message) = &s.cutoff_notice {
        cutoff = cutoff.child(label(message, BODY, color(FOREST)).wrap());
    }
    col()
        .key("settings-content")
        .w_full()
        .gap(16.0)
        .child(heading("Settings", 1))
        .child(cutoff)
        .child(aliases(s, compact, cx))
}

fn comparison(total: i64, previous: i64, today: bool) -> String {
    let period = if today {
        "yesterday"
    } else {
        "the previous day"
    };
    let delta = total - previous;
    if delta.abs() < 1_000 {
        format!("Same as {period} · {} recorded", duration(previous))
    } else {
        let sign = if delta > 0 { "+" } else { "−" };
        format!(
            "{sign}{} vs {period} · {} recorded",
            duration(delta.abs()),
            duration(previous)
        )
    }
}

fn timetable(s: &State, compact: bool) -> El {
    let mut log = panel()
        .key("timetable")
        .w_full()
        .child(heading("Timetable", 2))
        .child(
            search_field(
                "timetable-search",
                "Search apps, pages, or times",
                &s.timetable_search,
                State::edit_timetable_search,
            )
            .max_w(412.0),
        );
    let query = search_query(&s.timetable_search);
    let visible = s
        .sessions
        .iter()
        .filter(|session| session.meets_cutoff(s.cutoff));
    if visible.clone().next().is_none() {
        let message = if s.sessions.is_empty() {
            "No focus sessions for this date.".into()
        } else {
            format!("No sessions met the {}s cutoff.", s.cutoff)
        };
        return log
            .child(separator())
            .child(label(message, BODY, color(MUTED)).wrap());
    }
    let matched: Vec<_> = visible
        .filter_map(|session| {
            if session.app.matches(&query)
                || time(session.start).contains(&query)
                || time(session.end).contains(&query)
                || duration(session.end - session.start).contains(&query)
            {
                return Some((session, Vec::new()));
            }
            let titles: Vec<_> = s
                .timetable_titles
                .as_ref()
                .and_then(|titles| titles.get(&session.id))
                .into_iter()
                .flatten()
                .filter(|title| title.to_lowercase().contains(&query))
                .collect();
            (!titles.is_empty()).then_some((session, titles))
        })
        .collect();
    if matched.is_empty() {
        return log
            .child(separator())
            .child(empty_message("No matching sessions."));
    }
    if !compact {
        log = log.child(
            row()
                .center_y()
                .gap(8.0)
                .child(label("APP", LABEL, color(MUTED)).grow(1.0).min_w(0.0))
                .child(label("START / END", LABEL, color(MUTED)).w(143.0))
                .child(label("DURATION", LABEL, color(MUTED)).w(68.0)),
        );
    }
    for (session, titles) in matched {
        log = log
            .child(separator())
            .child(session_row(s, session, &titles, compact));
    }
    log
}

fn session_row(s: &State, session: &Session, matching_titles: &[&String], compact: bool) -> El {
    let id = session.id;
    let browser = session.app.id.browser_brand().is_some();
    let expanded = browser && s.expanded == Some(id);
    let mut app = row()
        .center_y()
        .min_w(0.0)
        .grow(1.0)
        .gap(8.0)
        .child(app_mark(s, &session.app.id.key))
        .child(
            label(session.app.label(), BODY, color(INK))
                .min_w(0.0)
                .truncate(),
        );
    if browser {
        app = app.child(label(if expanded { "⌄" } else { "›" }, BODY, color(MUTED)));
    }
    let time_row = row()
        .center_y()
        .gap(8.0)
        .child(
            label(
                format!("{}–{}", time(session.start), time(session.end)),
                TIME,
                color(MUTED),
            )
            .w(143.0),
        )
        .child(label(duration(session.end - session.start), TIME, color(INK)).w(68.0));
    let mut entry = if compact {
        col().gap(5.0)
    } else {
        row().center_y().gap(8.0)
    };
    entry = entry
        .key(format!("session:{id}"))
        .min_h(if browser { 44.0 } else { 34.0 })
        .child(app)
        .child(time_row);
    if browser {
        entry = entry
            .role("button")
            .label(format!("{} page titles", session.app.label()))
            .expanded(expanded)
            .focusable(true)
            .focus_ring(2.0, 2.0, color(FOREST))
            .radius(8.0)
            .hover(color(BASE))
            .on_press(move |s: &mut State| {
                s.expanded = (s.expanded != Some(id)).then_some(id);
                s.refresh_pages();
            });
    }
    let mut group = col().gap(6.0).child(entry);
    if expanded {
        if s.pages.is_empty() {
            group = group.child(
                label(
                    "No page titles recorded for this session.",
                    BODY,
                    color(MUTED),
                )
                .wrap(),
            );
        }
        for page in &s.pages {
            let item = if compact {
                col().gap(4.0)
            } else {
                row().center_y().gap(8.0)
            };
            group = group.child(
                item.key(format!("browser-page:{}", page.start))
                    .px(12.0)
                    .py(8.0)
                    .radius(8.0)
                    .fill(color(BASE))
                    .child(
                        label(&page.title, BODY, color(INK))
                            .min_w(0.0)
                            .grow(1.0)
                            .wrap(),
                    )
                    .child(label(
                        format!(
                            "{}–{}  ·  {}",
                            time(page.start),
                            time(page.end),
                            duration(page.end - page.start)
                        ),
                        TIME,
                        color(MUTED),
                    )),
            );
        }
    } else if !matching_titles.is_empty() {
        group = group.child(
            label(matching_titles[0], BODY, color(MUTED))
                .px(16.0)
                .truncate(),
        );
        if matching_titles.len() > 1 {
            group = group.child(
                label(
                    format!("{} more matching pages", matching_titles.len() - 1),
                    LABEL,
                    color(MUTED),
                )
                .px(16.0),
            );
        }
    }
    group
}

fn calendar(s: &State, compact: bool) -> El {
    let month_controls = row()
        .gap(6.0)
        .center_y()
        .child(button(
            "previous-month",
            "Previous",
            false,
            s.month <= s.first.with_day(1).unwrap(),
            |s, _| s.step_month(false),
        ))
        .child(button(
            "this-month",
            "This month",
            false,
            s.month == s.today.with_day(1).unwrap(),
            |s, _| s.show_month(s.today.with_day(1).unwrap()),
        ))
        .child(button(
            "next-month",
            "Next",
            false,
            s.month >= s.today.with_day(1).unwrap(),
            |s, _| s.step_month(true),
        ));
    let month_header = if compact {
        col().gap(8.0)
    } else {
        row().center_y().gap(12.0)
    };
    let month_header = month_header
        .child(heading(&s.month.format("%B %Y").to_string(), 2).grow(1.0))
        .child(month_controls);
    let weekdays =
        row()
            .w_full()
            .gap(4.0)
            .children(
                ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map(|name| {
                    el().w(0.0)
                        .grow(1.0)
                        .center()
                        .child(label(name, LABEL, color(MUTED)))
                }),
            );
    let offset = s.month.weekday().num_days_from_monday() as i64;
    let mut grid = col().w_full().gap(4.0).child(weekdays);
    for week in 0..6 {
        let mut days = row().w_full().gap(4.0);
        for column in 0..7 {
            let index = week * 7 + column - offset;
            let day = (index >= 0)
                .then(|| s.month.checked_add_days(Days::new(index as u64)))
                .flatten()
                .filter(|day| day.month() == s.month.month());
            days = days.child(match day {
                Some(date) => calendar_day(s, date, compact),
                None => el().w(0.0).grow(1.0),
            });
        }
        grid = grid.child(days);
    }
    col()
        .key("calendar-content")
        .w_full()
        .gap(16.0)
        .child(heading("Calendar", 1))
        .child(
            panel()
                .key("calendar-month")
                .padding(if compact { 10.0 } else { 22.0 })
                .child(month_header)
                .child(grid),
        )
}

fn calendar_day(s: &State, day: chrono::NaiveDate, compact: bool) -> El {
    let total = s.month_totals[day.day0() as usize];
    let active = day <= s.today;
    let selected = day == s.day;
    let name = format!("{}, {} recorded", day.format("%A, %B %-d"), duration(total));
    let tone = if selected {
        PAPER
    } else if active {
        INK
    } else {
        MUTED
    };
    let short_total = match total / 1000 {
        0..=59 => "<1m".into(),
        60..=3599 => format!("{}m", total / 60_000),
        _ => format!("{}h{:02}", total / 3_600_000, total / 60_000 % 60),
    };
    let mut tile = col()
        .key(format!("date:{day}"))
        .w(0.0)
        .grow(1.0)
        .min_h(if compact { 54.0 } else { 72.0 })
        .radius(8.0)
        .center()
        .gap(3.0)
        .fill(color(if selected {
            FOREST
        } else if total > 0 {
            0xddebd6
        } else {
            BASE
        }))
        .child(label(day.day().to_string(), BODY, color(tone)));
    if total > 0 {
        tile = tile.child(label(
            if compact {
                short_total
            } else {
                duration(total)
            },
            LABEL,
            color(tone),
        ));
    }
    if active {
        tile = tile
            .role("button")
            .label(&name)
            .help(name)
            .focusable(true)
            .focus_ring(2.0, 1.0, color(FOREST))
            .hover(color(if selected { 0x2b5543 } else { 0xcfe2c9 }))
            .on_press_tx(move |s: &mut State, tx| {
                s.show_screen(Screen::Activity);
                s.select_day(day);
                tx.scroll_to_instant("page", 0.0, 0.0);
            });
    }
    tile
}

fn pause_control(s: &State) -> El {
    let name = if s.paused {
        "Resume tracking"
    } else {
        "Pause tracking"
    };
    let icon = if s.paused {
        label("▶", TextStyle::new(15.0, 500.0), color(FOREST))
    } else {
        row()
            .center()
            .gap(3.0)
            .children([0, 1].map(|_| el().size(3.0, 13.0).radius(1.0).fill(color(FOREST))))
    };
    row()
        .center_y()
        .gap(8.0)
        .child(
            row()
                .key("pause")
                .role("button")
                .label(name)
                .help(name)
                .focusable(true)
                .focus_ring(2.0, 2.0, color(FOREST))
                .size(44.0, 44.0)
                .center()
                .radius(10.0)
                .fill(color(BASE))
                .hover(color(0xddebd6))
                .motion(motion::SNAPPY)
                .on_press(|s: &mut State| s.toggle_pause())
                .child(icon),
        )
        .child(label(
            if s.paused { "Paused" } else { "Tracking" },
            BODY,
            color(MUTED),
        ))
}

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let viewport = cx.layout_size("window");
    let compact = viewport.is_some_and(|[width, _]| width < 690.0);
    let navigation = row().center_y().gap(6.0).flex_wrap().children(
        [
            ("activity-tab", "Activity", Screen::Activity),
            ("calendar-tab", "Calendar", Screen::Calendar),
            ("reports-tab", "Reports", Screen::Reports),
            ("settings-tab", "Settings", Screen::Settings),
        ]
        .map(|(key, title, screen)| {
            button(key, title, s.screen == screen, false, move |s, tx| {
                s.show_screen(screen);
                tx.scroll_to_instant("page", 0.0, 0.0);
            })
        }),
    );
    let brand = row()
        .center_y()
        .gap(9.0)
        .grow(1.0)
        .child(
            row()
                .size(32.0, 32.0)
                .center()
                .gap(2.0)
                .radius(8.0)
                .fill(color(FOREST))
                .child(el().size(3.0, 8.0).radius(1.0).fill(color(LIME)))
                .child(el().size(3.0, 16.0).radius(1.0).fill(color(LIME)))
                .child(el().size(3.0, 12.0).radius(1.0).fill(color(LIME))),
        )
        .child(label(
            "Molo Tracker",
            TextStyle::new(17.0, 650.0).tracking(-0.4),
            color(INK),
        ));
    let header = if compact {
        col().gap(8.0)
    } else {
        row().center_y().gap(18.0)
    };
    let inset = if compact { 14.0 } else { 30.0 };

    let content = match s.screen {
        Screen::Activity => activity(s, compact),
        Screen::Calendar => calendar(s, compact),
        Screen::Reports => reports::view(s, compact),
        Screen::Settings => settings(s, compact, cx),
    };
    let mut page = col()
        .key("page-content")
        .w_full()
        .px(inset)
        .py(20.0)
        .gap(12.0)
        .child(content.transition(Transition::slide()));
    if let Some(message) = s
        .tracking_error
        .as_ref()
        .or_else(|| s.notice.as_ref().filter(|_| s.screen != Screen::Settings))
    {
        page = page.child(label(message, BODY, color(MUTED)).wrap());
    }
    col()
        .key("window")
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(color(BASE))
        .child(
            header
                .w_full()
                .px(inset)
                .py(12.0)
                .fill(color(SURFACE))
                .child(
                    row()
                        .center_y()
                        .gap(10.0)
                        .grow(1.0)
                        .child(brand)
                        .child(pause_control(s)),
                )
                .child(navigation),
        )
        .child(separator())
        .child(
            col()
                .key("page")
                .w_full()
                .min_h(0.0)
                .grow(1.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(page),
        )
        .children(viewport.and_then(|size| drag_ghost(s, size)))
}
