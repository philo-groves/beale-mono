//! Research desk: resume recent sessions and open workspaces. Every value shown is recorded
//! state already loaded for the sidebar; Home issues no cross-workspace queries.
use crate::design::FadeText;
use crate::workspace_creation::display_directory;
use crate::{State, ThemeStyle, design, form_button, icons::*, ui_font};
use beale_contracts::v1::{SessionSummary, WorkspaceSummary};
use rune::{El, ScrollSpec, col, el, icon, row, text};

/// Recent sessions listed under Continue research.
const RECENT_SESSION_LIMIT: usize = 5;
/// Narrowest workspace card before the grid drops to fewer columns.
const WORKSPACE_CARD_MIN_WIDTH: f32 = 300.0;
const WORKSPACE_GRID_GAP: f32 = 12.0;
const PAGE_PADDING: f32 = 36.0;
const PAGE_MAX_WIDTH: f32 = 1020.0;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Short activity label relative to `now_ms`; `None` when the time was never recorded.
pub(crate) fn relative_time(at_ms: u64, now_ms: u64) -> Option<String> {
    if at_ms == 0 {
        return None;
    }
    let minutes = now_ms.saturating_sub(at_ms) / 60_000;
    Some(match minutes {
        0 => "Just now".to_owned(),
        1..=59 => format!("{minutes}m ago"),
        60..=1_439 => format!("{}h ago", minutes / 60),
        1_440..=2_879 => "Yesterday".to_owned(),
        2_880..=10_079 => format!("{}d ago", minutes / 1_440),
        _ => chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            at_ms.min(i64::MAX as u64) as i64
        )
        .map_or_else(
            || "Earlier".to_owned(),
            |date| {
                date.with_timezone(&chrono::Local)
                    .format("%b %-d")
                    .to_string()
            },
        ),
    })
}

/// Sessions with the most recent activity first; unrecorded times sort last, in list order.
fn recent_sessions(sessions: &[SessionSummary]) -> Vec<&SessionSummary> {
    let mut recent: Vec<_> = sessions.iter().collect();
    recent.sort_by_key(|session| std::cmp::Reverse(session.last_message_at_ms));
    recent.truncate(RECENT_SESSION_LIMIT);
    recent
}

/// Workspace grid columns for a content width.
fn workspace_columns(content_width: f32) -> usize {
    let fit = ((content_width + WORKSPACE_GRID_GAP)
        / (WORKSPACE_CARD_MIN_WIDTH + WORKSPACE_GRID_GAP))
        .floor() as usize;
    fit.clamp(1, 2)
}

fn section_heading(label: &str, count: Option<usize>, theme: ThemeStyle<'_>) -> El {
    let mut heading = row().w_full().center_y().gap(8.0).child(
        text(label)
            .font(ui_font(16.0, 600.0))
            .color(theme.color("#f2f2f2")),
    );
    if let Some(count) = count {
        heading = heading.child(
            text(count.to_string())
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#969696")),
        );
    }
    heading
}

fn session_row(
    session: &SessionSummary,
    workspace: Option<&WorkspaceSummary>,
    now: u64,
    first: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let id = session.id.clone();
    let mut details = row().w_full().min_w(0.0).gap(6.0).center_y().child(
        text(workspace.map_or("Unknown workspace".to_owned(), |item| item.name.clone()))
            .min_w(0.0)
            .shrink()
            .fade_truncate()
            .font(ui_font(12.0, 400.0))
            .color(theme.color("#969696")),
    );
    if let Some(when) = relative_time(session.last_message_at_ms, now) {
        details = details
            .child(
                text("·")
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#6f6f6f")),
            )
            .child(
                text(when)
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#969696")),
            );
    }
    let mut item = row()
        .key(format!("overview-session-{}", session.id))
        .role("button")
        .label(format!("Continue {}", session.title))
        .w_full()
        .px(14.0)
        .py(11.0)
        .gap(12.0)
        .center_y()
        .radius(10.0)
        .continuous_corners()
        .hover(theme.color("#242424"))
        .child(
            el().size(32.0, 32.0)
                .center()
                .radius(9.0)
                .continuous_corners()
                .fill(theme.subtle_accent())
                .child(icon(&CHAT_BUBBLE, 16.0).color(theme.accent())),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(3.0)
                .child(
                    text(session.title.clone())
                        .w_full()
                        .fade_truncate()
                        .help_when_truncated(session.title.clone())
                        .font(ui_font(14.0, 550.0))
                        .color(theme.color("#ededed")),
                )
                .child(details),
        )
        .child(icon(&CHEVRON_RIGHT, 15.0).color(theme.color("#7a7a7a")))
        .on_press(move |state: &mut State| {
            if state
                .active_session
                .as_ref()
                .and_then(|detail| detail.summary.as_ref())
                .is_some_and(|summary| summary.id == id)
            {
                state.navigate_to(crate::ViewLocation::NewResearch);
            } else {
                crate::start_session_load(state, id.clone());
            }
        });
    if !first {
        item = item.child(
            el().pin(Some(14.0), Some(0.0), Some(14.0), None)
                .h(1.0)
                .fill(theme.color("#262626"))
                .inert(),
        );
    }
    item
}

fn workspace_card(
    workspace: &WorkspaceSummary,
    sessions: &[SessionSummary],
    columns: usize,
    now: u64,
    theme: ThemeStyle<'_>,
) -> Option<El> {
    let id = uuid::Uuid::parse_str(&workspace.id).ok()?;
    let owned: Vec<_> = sessions
        .iter()
        .filter(|session| session.workspace_id == workspace.id)
        .collect();
    let count = owned.len();
    let last = owned
        .iter()
        .map(|session| session.last_message_at_ms)
        .max()
        .unwrap_or(0);
    let mut activity = format!("{count} {}", if count == 1 { "scan" } else { "scans" });
    if let Some(when) = relative_time(last, now) {
        activity.push_str(&format!(" · Active {}", when.to_lowercase()));
    }
    let directory = display_directory(&workspace.directory);
    Some(
        col()
            .key(format!("overview-workspace-{id}"))
            .role("button")
            .label(format!("Open {}", workspace.name))
            .w_frac(1.0 / columns as f32)
            .shrink()
            .min_w(0.0)
            .padding(16.0)
            .gap(12.0)
            .radius(design::CARD_RADIUS)
            .continuous_corners()
            .fill(theme.color("#191919"))
            .border(1.0, theme.color("#2a2a2a"))
            .hover(theme.color("#212121"))
            .on_press(move |s: &mut State| s.open_workspace(id))
            .child(
                row()
                    .w_full()
                    .gap(12.0)
                    .center_y()
                    .child(
                        el().size(36.0, 36.0)
                            .center()
                            .radius(10.0)
                            .continuous_corners()
                            .fill(theme.subtle_accent())
                            .child(crate::workspace_logo::mark(
                                Some(workspace),
                                18.0,
                                theme.accent(),
                            )),
                    )
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(3.0)
                            .child(
                                text(workspace.name.clone())
                                    .w_full()
                                    .fade_truncate()
                                    .help_when_truncated(workspace.name.clone())
                                    .font(ui_font(15.0, 600.0))
                                    .color(theme.color("#f2f2f2")),
                            )
                            .child(
                                text(directory.clone())
                                    .w_full()
                                    .fade_truncate()
                                    .help_when_truncated(directory)
                                    .font(ui_font(12.0, 400.0))
                                    .color(theme.color("#8a8a8a")),
                            ),
                    )
                    .child(icon(&CHEVRON_RIGHT, 15.0).color(theme.color("#7a7a7a"))),
            )
            .child(el().w_full().h(1.0).fill(theme.color("#262626")))
            .child(
                text(activity)
                    .w_full()
                    .fade_truncate()
                    .font(ui_font(12.0, 500.0))
                    .color(theme.color("#a4a4a4")),
            ),
    )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, page_width: f32) -> El {
    let now = now_ms();
    let content_width = (page_width.min(PAGE_MAX_WIDTH) - PAGE_PADDING * 2.0).max(0.0);
    // Top-level pages open on their content: the sidenav header already names Home.
    let header = row()
        .key("overview-header")
        .gap(10.0)
        .child(
            design::primary_button("New Scan", theme)
                .key("overview-new-research")
                .on_press(State::start_new_research),
        )
        .child(
            form_button("Create workspace", theme)
                .key("overview-new-workspace")
                .on_press(State::start_workspace_creation),
        );
    let mut page = col()
        .w_full()
        .max_w(PAGE_MAX_WIDTH)
        .self_center()
        .padding(PAGE_PADDING)
        .gap(design::PAGE_GAP)
        .child(header);
    if state.workspaces.is_empty() {
        page = page.child(design::empty_state(
            &FOLDER_OPEN,
            "Set the boundary for your research",
            "Create a workspace to keep authorized scope, scans, surfaces, and findings together. Start with a directory and a scope rule.",
            theme,
        ));
        return scroll(page, state, theme);
    }

    let recent = recent_sessions(&state.session_summaries);
    let mut continue_section = col()
        .key("overview-recent")
        .w_full()
        .gap(10.0)
        .child(section_heading("Continue research", None, theme));
    if recent.is_empty() {
        continue_section = continue_section.child(
            row()
                .key("overview-recent-empty")
                .w_full()
                .padding(16.0)
                .radius(design::CARD_RADIUS)
                .continuous_corners()
                .border(1.0, theme.color("#2a2a2a"))
                .child(design::subtitle(
                    "No scans yet. Start New Scan to open the first one.",
                    theme,
                )),
        );
    } else {
        let mut list = col()
            .key("overview-recent-list")
            .w_full()
            .padding(4.0)
            .radius(design::CARD_RADIUS)
            .continuous_corners()
            .fill(theme.color("#191919"))
            .border(1.0, theme.color("#2a2a2a"));
        for (index, session) in recent.into_iter().enumerate() {
            let workspace = state
                .workspaces
                .iter()
                .find(|item| item.id == session.workspace_id);
            list = list.child(session_row(session, workspace, now, index == 0, theme));
        }
        continue_section = continue_section.child(list);
    }
    page = page.child(continue_section);

    let columns = workspace_columns(content_width);
    let mut grid = col()
        .key("overview-workspaces-grid")
        .w_full()
        .gap(WORKSPACE_GRID_GAP);
    let cards: Vec<El> = state
        .workspaces
        .iter()
        .filter_map(|workspace| {
            workspace_card(workspace, &state.session_summaries, columns, now, theme)
        })
        .collect();
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut line = row().w_full().gap(WORKSPACE_GRID_GAP);
        for _ in 0..columns {
            // Keep a trailing odd card at column width rather than stretching it across the row.
            line = line.child(
                cards
                    .next()
                    .unwrap_or_else(|| el().w_frac(1.0 / columns as f32).shrink().min_w(0.0)),
            );
        }
        grid = grid.child(line);
    }
    page = page.child(
        col()
            .key("overview-workspaces")
            .w_full()
            .gap(10.0)
            .child(section_heading(
                "Workspaces",
                Some(state.workspaces.len()),
                theme,
            ))
            .child(grid),
    );
    scroll(page, state, theme)
}

fn scroll(page: El, state: &State, theme: ThemeStyle<'_>) -> El {
    let scroll_watch = state.home_scroll.clone();
    let scroll_bar = scroll_watch.clone();
    col()
        .key("home-overview-shell")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(
            col()
                .key("home-overview")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .scroll_edge_fade(16.0)
                .on_scroll_offset(move |position| scroll_watch.watch_scroll(position))
                .on_wheel(|state: &mut State, event, tx| state.home_scroll.wheel(event, tx))
                .child(page),
        )
        .child(scroll_bar.bar(theme))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_labels_recent_activity() {
        let now = 10 * 86_400_000;
        assert_eq!(relative_time(0, now), None);
        assert_eq!(
            relative_time(now - 20_000, now).as_deref(),
            Some("Just now")
        );
        assert_eq!(
            relative_time(now - 5 * 60_000, now).as_deref(),
            Some("5m ago")
        );
        assert_eq!(
            relative_time(now - 3 * 3_600_000, now).as_deref(),
            Some("3h ago")
        );
        assert_eq!(
            relative_time(now - 30 * 3_600_000, now).as_deref(),
            Some("Yesterday")
        );
        assert_eq!(
            relative_time(now - 4 * 86_400_000, now).as_deref(),
            Some("4d ago")
        );
        assert!(relative_time(now - 9 * 86_400_000, now).is_some());
    }

    #[test]
    fn recent_sessions_are_newest_first_and_bounded() {
        let sessions: Vec<_> = (0..8u64)
            .map(|index| SessionSummary {
                id: format!("session-{index}"),
                last_message_at_ms: index * 1_000,
                ..Default::default()
            })
            .collect();
        let recent = recent_sessions(&sessions);
        assert_eq!(recent.len(), RECENT_SESSION_LIMIT);
        assert_eq!(recent[0].id, "session-7");
        assert_eq!(recent[4].id, "session-3");
    }

    #[test]
    fn workspace_grid_uses_two_columns_only_when_cards_fit() {
        assert_eq!(workspace_columns(900.0), 2);
        assert_eq!(workspace_columns(612.0), 2);
        assert_eq!(workspace_columns(560.0), 1);
        assert_eq!(workspace_columns(0.0), 1);
    }
}
