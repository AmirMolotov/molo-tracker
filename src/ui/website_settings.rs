use super::*;
use crate::store::Website;
use crate::websites::parent_domain;

pub(super) fn view(s: &State, compact: bool, cx: &Cx<State>) -> El {
    let query = search_query(&s.website_search);
    let targets = GroupDropTargets::new(s, cx);
    let parents: std::collections::BTreeSet<_> = s
        .websites
        .iter()
        .filter_map(|site| site.parent.as_deref())
        .collect();
    let mut entries = col().gap(3.0);
    let matching = s.websites.iter().filter(|site| {
        s.website_folder.as_ref().is_none_or(|group| {
            site.group
                .as_ref()
                .map_or_else(String::new, |name| name.to_lowercase())
                == *group
        }) && (site.domain.to_lowercase().contains(&query)
            || site
                .group
                .as_ref()
                .is_some_and(|name| name.to_lowercase().contains(&query)))
    });
    if matching.clone().next().is_none() {
        entries = entries.child(empty_message(if s.websites.is_empty() {
            "Domains appear here after Molo records website activity in Chrome or Firefox."
        } else {
            "No websites match this group or search."
        }));
    }
    for site in matching {
        let selected = s.website_group_domain.as_ref() == Some(&site.domain);
        let lifted = s
            .drag_preview
            .as_ref()
            .is_some_and(|drag| drag.source == site.domain);
        let name = site.group.as_deref().unwrap_or("Ungrouped");
        let domain = site.domain.clone();
        let drag = group_drag(domain.clone(), targets.clone());
        let whole_domain = parents.contains(site.domain.as_str());
        entries = entries.child(
            row()
                .key(format!("settings-domain:{}", site.domain))
                .role("button")
                .label(format!("{}, {}", site.domain, name))
                .checked(selected)
                .focusable(true)
                .focus_ring(2.0, -2.0, color(FOREST))
                .min_h(44.0)
                .px(8.0)
                .center_y()
                .gap(8.0)
                .radius(8.0)
                .fill(color(if selected { 0xddebd6 } else { SURFACE }))
                .hover(color(BASE))
                .motion(motion::SNAPPY)
                .offset(0.0, if lifted { -2.0 } else { 0.0 })
                .opacity(if lifted { 0.56 } else { 1.0 })
                .child(
                    col()
                        .min_w(0.0)
                        .grow(1.0)
                        .gap(2.0)
                        .child(
                            label(&site.domain, BODY, color(INK))
                                .truncate()
                                .help_when_truncated(&site.domain),
                        )
                        .children(
                            whole_domain.then(|| label("All subdomains", LABEL, color(MUTED))),
                        ),
                )
                .child(
                    label(name, LABEL, color(MUTED))
                        .max_w(112.0)
                        .truncate()
                        .help_when_truncated(name),
                )
                .on_press_tx(move |s: &mut State, tx| {
                    s.select_website(&domain);
                    if compact {
                        tx.scroll_into_view("website-settings-group");
                    }
                })
                .on_drag(drag),
        );
    }
    let list = panel()
        .grow(1.0)
        .min_w(0.0)
        .child(empty_message(
            "Drag a website onto a group. Drag the main domain to move all its subdomains.",
        ))
        .child(group_folders(s))
        .child(search_field(
            "settings-website-search",
            "Search domains or groups",
            &s.website_search,
            |s| &mut s.website_search,
        ))
        .child(
            col()
                .key("settings-website-list")
                .h(if compact { 190.0 } else { 300.0 })
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(entries),
        );
    let selected = s
        .websites
        .iter()
        .find(|site| s.website_group_domain.as_ref() == Some(&site.domain));
    let mut editor = panel().key("website-settings-group").grow(1.0).min_w(0.0);
    editor = match selected {
        Some(site) => editor.child(group_editor(s, site)),
        None => editor.child(empty_message("Select a domain to edit its group.")),
    };
    editor = editor.children(s.website_notice.as_deref().map(empty_message));
    let layout = if compact { col() } else { row().items_start() };
    layout.gap(14.0).child(list).child(editor)
}

fn group_editor(s: &State, site: &Website) -> El {
    let inherited = (!site.has_assignment && site.group.is_some())
        .then(|| parent_domain(&site.domain))
        .flatten();
    col()
        .gap(9.0)
        .child(heading(&site.domain, 2).wrap())
        .children(
            site.parent.is_none().then(|| {
                empty_message("Group changes apply to this domain and all its subdomains.")
            }),
        )
        .children(
            inherited.map(|parent| empty_message(&format!("Inherits its group from {parent}."))),
        )
        .child(label("Group name", BODY, color(MUTED)))
        .child(
            el().w_full()
                .h(44.0)
                .px(10.0)
                .center_y()
                .radius(8.0)
                .border(1.0, color(LINE))
                .fill(color(BASE))
                .child(
                    text_input(&s.website_group_input, BODY)
                        .key("website-group-input")
                        .label("Group name")
                        .single_line()
                        .color(color(INK))
                        .w_full()
                        .on_edit(|s: &mut State| &mut s.website_group_input)
                        .on_submit(|s: &mut State, _| s.save_website_group()),
                ),
        )
        .child(
            row()
                .gap(8.0)
                .flex_wrap()
                .child(button(
                    "save-website-group",
                    "Save group",
                    true,
                    s.website_group_input.text.trim().is_empty(),
                    |s, _| {
                        s.save_website_group();
                    },
                ))
                .children((!s.folders.is_empty()).then(|| {
                    let domain = site.domain.clone();
                    button(
                        "choose-website-group",
                        "Choose existing",
                        false,
                        false,
                        move |s, tx| {
                            let menu =
                                rune::PopupMenu::new().items(s.folders.values().map(|name| {
                                    rune::PopupItem::new(name, name.replace('&', "&&"))
                                }));
                            let domain = domain.clone();
                            tx.popup_menu(
                                menu,
                                "choose-website-group",
                                move |s: &mut State, picked, _| {
                                    if let Some(name) = picked
                                        && s.website_group_domain.as_ref() == Some(&domain)
                                    {
                                        s.update_website_group(Some(name));
                                    }
                                },
                            );
                        },
                    )
                }))
                .children(site.group.as_ref().map(|_| {
                    button("ungroup-website", "Ungroup", false, false, |s, _| {
                        s.update_website_group(Some(""));
                    })
                }))
                .children(
                    (site.has_assignment && parent_domain(&site.domain).is_some()).then(|| {
                        button(
                            "inherit-website-group",
                            "Use parent group",
                            false,
                            false,
                            |s, _| {
                                s.update_website_group(None);
                            },
                        )
                    }),
                ),
        )
}
