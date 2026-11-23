use std::f32::consts::{FRAC_PI_2, TAU};

use rune::Shape;

use super::*;
use crate::report::{self, GroupUsage, Period};

pub(super) fn view(s: &State, compact: bool) -> El {
    let period = s.report_period;
    let start = period.start(s.report_date);
    let end = period.adjacent(start, true).unwrap();
    let previous = period.adjacent(start, false).unwrap();
    let current = period.start(s.today);
    let dates = report::date_range(start, end.min(s.today.succ_opt().unwrap()));
    let baseline = if period == Period::Daily && s.report_date == s.today {
        format!(
            "Compared with yesterday · {}",
            report::date_range(previous, start)
        )
    } else {
        format!("Compared with {}", report::date_range(previous, start))
    };
    let switch = row().gap(6.0).flex_wrap().children(
        [
            ("daily", Period::Daily),
            ("weekly", Period::Weekly),
            ("monthly", Period::Monthly),
        ]
        .map(|(key, option)| {
            button(key, option.label(), period == option, false, move |s, _| {
                s.report_period = option;
                s.report_group = None;
                s.refresh_report();
            })
        }),
    );
    let navigation = row()
        .gap(6.0)
        .flex_wrap()
        .child(button(
            "report-prev",
            "Previous",
            false,
            start <= period.start(s.first),
            |s, _| s.step_report(false),
        ))
        .child(button(
            "report-current",
            "Current",
            false,
            start == current,
            |s, _| {
                s.report_date = s.today;
                s.report_group = None;
                s.refresh_report();
            },
        ))
        .child(button(
            "report-next",
            "Next",
            false,
            start >= current,
            |s, _| s.step_report(true),
        ));
    let controls = if compact {
        col().gap(8.0)
    } else {
        row().center_y().gap(8.0)
    };
    let controls = controls
        .child(el().grow(1.0).child(switch))
        .child(navigation);
    let report_views = row()
        .gap(6.0)
        .flex_wrap()
        .child(button(
            "reports-apps-view",
            "Apps and groups",
            !s.report_websites,
            false,
            |s, _| {
                s.report_websites = false;
            },
        ))
        .child(button(
            "reports-websites-view",
            "Websites",
            s.report_websites,
            false,
            |s, _| {
                s.report_websites = true;
            },
        ));

    let total = s.report.total;
    let prior = s.report.previous;
    let metrics = if compact {
        col().gap(12.0)
    } else {
        row().center_y().gap(30.0)
    };
    let summary = col()
        .key("report-summary")
        .w_full()
        .radius(15.0)
        .fill(color(FOREST))
        .padding(22.0)
        .child(
            metrics
                .child(metric(
                    "FOCUS TIME",
                    duration(total.time),
                    time_change(total.time - prior.time),
                ))
                .child(metric(
                    "FOCUS SESSIONS",
                    total.sessions.to_string(),
                    signed(total.sessions - prior.sessions),
                )),
        );

    let report = col()
        .key("reports-content")
        .w_full()
        .gap(16.0)
        .child(
            col()
                .gap(3.0)
                .child(heading("Reports", 1))
                .child(label(dates, BODY, color(INK)).wrap())
                .child(label(baseline, BODY, color(MUTED)).wrap()),
        )
        .child(controls)
        .child(report_views);
    if s.report_websites {
        report.child(websites(s, compact))
    } else {
        report
            .child(summary)
            .child(groups(s, compact))
            .child(apps(s, compact))
    }
}

fn websites(s: &State, compact: bool) -> El {
    let domains = &s.report.websites;
    let total: i64 = domains.iter().map(|site| site.total()).sum();
    let previous: i64 = domains.iter().map(|site| site.previous).sum();
    let mut card = panel()
        .key("report-websites")
        .child(heading("Websites by domain", 2))
        .child(
            search_field(
                "report-website-search",
                "Search domains",
                &s.report_website_search,
                |s| &mut s.report_website_search,
            )
            .max_w(412.0),
        );
    if domains.is_empty() {
        return card.child(label("No website time recorded for these dates. New Chrome and Firefox activity will appear here when a domain is available.", BODY, color(MUTED)).wrap());
    }

    card = card.child(
        row()
            .gap(8.0)
            .center_y()
            .flex_wrap()
            .child(label(
                format!("{} recorded", duration(total)),
                BODY,
                color(INK),
            ))
            .child(label(
                format!("{} vs previous period", time_change(total - previous)),
                TIME,
                color(MUTED),
            )),
    );
    let query = search_query(&s.report_website_search);
    let matching = domains
        .iter()
        .filter(|site| site.domain.to_lowercase().contains(&query));
    if matching.clone().next().is_none() {
        return card.child(empty_message("No matching domains."));
    }
    if !compact {
        card = card.child(
            row()
                .gap(10.0)
                .child(label("DOMAIN", LABEL, color(MUTED)).min_w(0.0).grow(1.0))
                .child(label("CHROME", LABEL, color(MUTED)).w(94.0))
                .child(label("FIREFOX", LABEL, color(MUTED)).w(94.0))
                .child(label("TOTAL / CHANGE", LABEL, color(MUTED)).w(190.0)),
        );
    }
    for site in matching {
        let time = format!(
            "{}  ({})",
            duration(site.total()),
            time_change(site.total() - site.previous)
        );
        let domain = label(&site.domain, BODY, color(INK))
            .min_w(0.0)
            .grow(1.0)
            .truncate()
            .help_when_truncated(&site.domain);
        let entry = if compact {
            col().gap(5.0).child(domain).child(
                label(
                    format!(
                        "{}  ·  Chrome {}  /  Firefox {}",
                        time,
                        duration(site.chrome),
                        duration(site.firefox)
                    ),
                    TIME,
                    color(MUTED),
                )
                .wrap(),
            )
        } else {
            row()
                .center_y()
                .gap(10.0)
                .min_h(34.0)
                .child(domain)
                .child(label(duration(site.chrome), TIME, color(MUTED)).w(94.0))
                .child(label(duration(site.firefox), TIME, color(MUTED)).w(94.0))
                .child(label(time, TIME, color(INK)).w(190.0))
        };
        card = card
            .child(separator())
            .child(entry.key(format!("domain:{}", site.domain)));
    }
    card
}

fn metric(title: &str, value: String, change: String) -> El {
    col()
        .grow(1.0)
        .gap(5.0)
        .child(label(title, LABEL, color(PALE)))
        .child(
            row()
                .items_end()
                .gap(10.0)
                .flex_wrap()
                .child(label(
                    value,
                    TextStyle::new(36.0, 550.0).tracking(-1.4),
                    color(LIME),
                ))
                .child(label(change, TIME, color(PAPER)).pb(5.0)),
        )
}

fn groups(s: &State, compact: bool) -> El {
    let total = s.report.total.time;
    let mut card = panel()
        .key("report-groups")
        .child(heading("Time by group", 2));
    if s.report.groups.is_empty() {
        return card.child(label("No focus sessions in either period.", BODY, color(MUTED)).wrap());
    }

    let mut legend = col().min_w(0.0).grow(1.0).gap(5.0);
    for (index, group) in s.report.groups.iter().enumerate() {
        legend = legend.child(group_row(s, group, index, total));
    }
    if total > 0 {
        let size = if compact { 174.0 } else { 206.0 };
        let chart = col().center().child(
            el().key("group-pie")
                .size(size, size)
                .shape(pie(&s.report.groups, total)),
        );
        let layout = if compact { col() } else { row().center_y() };
        card = card.child(layout.gap(18.0).child(chart).child(legend));
    } else {
        card = card
            .child(label("No focus time recorded in this period.", BODY, color(MUTED)).wrap())
            .child(legend);
    }
    card
}

fn group_row(s: &State, group: &GroupUsage, index: usize, total: i64) -> El {
    let key = group.key.clone();
    let selected = s.report_group.as_ref() == Some(&key);
    let share = if total == 0 {
        0.0
    } else {
        group.current.time as f64 * 100.0 / total as f64
    };
    let change = time_change(group.current.time - group.previous.time);
    let count = group.current.sessions;
    let noun = if count == 1 { "session" } else { "sessions" };
    let session_change = signed(count - group.previous.sessions);
    let name = format!(
        "{}, {:.1}%, {}, {count} {noun}",
        group.name,
        share,
        duration(group.current.time)
    );
    row()
        .key(format!("report-group:{key}"))
        .role("button")
        .label(name)
        .focusable(true)
        .focus_ring(2.0, 1.0, color(FOREST))
        .radius(8.0)
        .px(8.0)
        .py(6.0)
        .min_h(44.0)
        .center_y()
        .gap(8.0)
        .fill(color(if selected { 0xddebd6 } else { SURFACE }))
        .hover(color(if selected { 0xcfe2c9 } else { BASE }))
        .on_press_tx(move |s: &mut State, tx| {
            s.report_group = (s.report_group.as_ref() != Some(&key)).then(|| key.clone());
            tx.scroll_into_view("report-apps");
        })
        .child(el().size(12.0, 12.0).radius(3.0).fill(group_color(index)))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(label(&group.name, BODY, color(INK)).truncate())
                .child(
                    label(
                        format!("{count} {noun} ({session_change})"),
                        TIME,
                        color(MUTED),
                    )
                    .wrap(),
                ),
        )
        .child(
            col()
                .items_end()
                .child(label(
                    format!("{:.1}% · {}", share, duration(group.current.time)),
                    TIME,
                    color(INK),
                ))
                .child(label(change, TIME, color(MUTED))),
        )
        .child(label("›", BODY, color(MUTED)))
}

fn group_color(index: usize) -> Color {
    let colors = [
        0x315f49, 0x969b51, 0x3f7078, 0xab7542, 0x705345, 0x769d7d, 0x526381, 0xc3926d,
    ];
    color(colors[index % colors.len()])
}

fn pie(groups: &[GroupUsage], total: i64) -> Shape {
    let mut shape = Shape::new().view_box(200.0, 200.0);
    let mut offset = 0;
    for (index, group) in groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.current.time > 0)
    {
        let start = TAU * (offset as f32 / total as f32) - FRAC_PI_2;
        offset += group.current.time;
        let end = TAU * (offset as f32 / total as f32) - FRAC_PI_2;
        let steps = ((end - start) * 32.0).ceil().max(1.0) as usize;
        let mut points = Vec::with_capacity(steps + 2);
        points.push([100.0, 100.0]);
        for step in 0..=steps {
            let a = start + (end - start) * step as f32 / steps as f32;
            points.push([100.0 + 98.0 * a.cos(), 100.0 + 98.0 * a.sin()]);
        }
        shape = shape.fill(points, group_color(index));
    }
    shape
}

fn apps(s: &State, compact: bool) -> El {
    let selected = s
        .report
        .groups
        .iter()
        .find(|g| Some(&g.key) == s.report_group.as_ref());
    let title = selected.map_or_else(
        || "App comparison".into(),
        |group| format!("Apps in {}", group.name),
    );
    let mut header = row()
        .center_y()
        .gap(8.0)
        .flex_wrap()
        .child(heading(&title, 2).grow(1.0));
    if selected.is_some() {
        header = header.child(button(
            "all-report-apps",
            "Show all apps",
            false,
            false,
            |s, _| s.report_group = None,
        ));
    }
    let mut card = panel().key("report-apps").child(header).child(
        search_field(
            "report-app-search",
            "Search report apps",
            &s.report_app_search,
            |s| &mut s.report_app_search,
        )
        .max_w(412.0),
    );
    if s.report.apps.is_empty() {
        return card.child(label("No app activity to compare.", BODY, color(MUTED)).wrap());
    }
    let query = search_query(&s.report_app_search);
    let matching = s.report.apps.iter().filter(|usage| {
        selected.is_none_or(|group| report::group_key(&usage.app) == group.key)
            && usage.app.matches(&query)
    });
    if matching.clone().next().is_none() {
        return card.child(empty_message("No matching apps."));
    }
    if !compact {
        card = card.child(
            row()
                .gap(10.0)
                .child(label("APP", LABEL, color(MUTED)).grow(1.0))
                .child(label("FOCUS TIME / CHANGE", LABEL, color(MUTED)).w(215.0))
                .child(label("SESSIONS / CHANGE", LABEL, color(MUTED)).w(160.0)),
        );
    }
    for usage in matching {
        let time = format!(
            "{}  ({})",
            duration(usage.current.time),
            time_change(usage.current.time - usage.previous.time)
        );
        let sessions = format!(
            "{}  ({})",
            usage.current.sessions,
            signed(usage.current.sessions - usage.previous.sessions)
        );
        let entry = if compact {
            col().gap(6.0)
        } else {
            row().center_y().gap(10.0)
        };
        let entry = entry
            .key(format!("report-app:{}", usage.app.id.key))
            .min_h(32.0)
            .child(
                row()
                    .center_y()
                    .gap(9.0)
                    .min_w(0.0)
                    .grow(1.0)
                    .child(app_mark(s, &usage.app.id.key))
                    .child(
                        label(usage.app.label(), BODY, color(INK))
                            .truncate()
                            .min_w(0.0)
                            .grow(1.0),
                    ),
            );
        card = card.child(separator()).child(if compact {
            entry
                .child(label(format!("Focus  {time}"), TIME, color(MUTED)))
                .child(label(format!("Sessions  {sessions}"), TIME, color(MUTED)))
        } else {
            entry
                .child(label(time, TIME, color(INK)).w(215.0))
                .child(label(sessions, TIME, color(INK)).w(160.0))
        });
    }
    card
}

fn time_change(ms: i64) -> String {
    match ms {
        0 => "0s".into(),
        positive if positive > 0 => format!("+{}", duration(ms)),
        _ => format!("−{}", duration(ms.abs())),
    }
}

fn signed(value: i64) -> String {
    if value == 0 {
        "0".into()
    } else if value > 0 {
        format!("+{value}")
    } else {
        format!("−{}", value.abs())
    }
}
