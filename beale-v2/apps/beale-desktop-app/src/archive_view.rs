use beale_contracts::{
    ARCHIVE_PATH, PROTOBUF_CONTENT_TYPE, QUICK_CHAT_DETAIL_PATH, SESSION_RESTORE_PATH,
    v1::{ArchiveEntry, ListArchiveResponse, QuickChatDetail},
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{El, col, el, row, text};
use std::{collections::HashSet, net::SocketAddr};

use crate::{
    State, ThemeStyle, form_button, provider_auth, quick_chat, start_archive_restore,
    start_quick_chat_restore, ui_font,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ArchiveTab {
    #[default]
    Sessions,
    QuickChats,
}

impl ArchiveTab {
    const ALL: [Self; 2] = [Self::Sessions, Self::QuickChats];

    fn label(self) -> &'static str {
        match self {
            Self::Sessions => "Sessions",
            Self::QuickChats => "Quick Chats",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Sessions => "archive-tab-sessions",
            Self::QuickChats => "archive-tab-quick-chats",
        }
    }
}

fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

async fn decode<T: Message + Default>(response: reqwest::Response) -> Result<T, String> {
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(format!("Archive request failed ({})", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read archive response".to_owned())?;
    if bytes.len() > 1_000_000 {
        return Err("Archive response is too large".to_owned());
    }
    T::decode(bytes).map_err(|_| "Invalid archive response".to_owned())
}

pub(crate) async fn list(client: &Client, addr: SocketAddr) -> Result<Vec<ArchiveEntry>, String> {
    let token = provider_auth::authenticated_token(client, addr).await?;
    let response: ListArchiveResponse = decode(
        client
            .get(url(addr, ARCHIVE_PATH))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load archive".to_owned())?,
    )
    .await?;
    let mut ids = HashSet::new();
    if response.entries.len() > 1_024
        || !response.entries.iter().all(|entry| {
            uuid::Uuid::parse_str(&entry.id).is_ok()
                && (entry.quick_chat && entry.workspace_id.is_empty()
                    || uuid::Uuid::parse_str(&entry.workspace_id).is_ok())
                && !entry.title.trim().is_empty()
                && entry.title.len() <= 256
                && !entry.model.trim().is_empty()
                && entry.model.len() <= 256
                && (1..=32).contains(&entry.turn_count)
                && entry.created_at_ms > 0
                && entry.last_message_at_ms > 0
                && entry.archived_at_ms >= entry.created_at_ms
                && ids.insert(entry.id.clone())
        })
    {
        return Err("Invalid archive response".to_owned());
    }
    Ok(response.entries)
}

pub(crate) async fn restore_session(
    client: &Client,
    addr: SocketAddr,
    id: &str,
) -> Result<(), String> {
    let token = provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(url(addr, &SESSION_RESTORE_PATH.replace("{id}", id)))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not restore session".to_owned())?;
    match response.status() {
        StatusCode::NO_CONTENT => Ok(()),
        StatusCode::NOT_FOUND => Err("Session is no longer available".to_owned()),
        _ => Err(format!("Could not restore session ({})", response.status())),
    }
}

pub(crate) async fn load_quick_chat(
    client: &Client,
    addr: SocketAddr,
    id: &str,
) -> Result<QuickChatDetail, String> {
    let token = provider_auth::authenticated_token(client, addr).await?;
    let detail: QuickChatDetail = decode(
        client
            .get(url(addr, &QUICK_CHAT_DETAIL_PATH.replace("{id}", id)))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load Quick Chat".to_owned())?,
    )
    .await?;
    if detail.chat_id != id || !quick_chat::valid_detail(&detail, &detail.workspace_id) {
        return Err("Invalid Quick Chat response".to_owned());
    }
    Ok(detail)
}

fn tab_button(tab: ArchiveTab, selected: bool, theme: ThemeStyle<'_>) -> El {
    row()
        .key(tab.key())
        .role("tab")
        .label(format!(
            "{} archive view{}",
            tab.label(),
            if selected { ", selected" } else { "" }
        ))
        .h(26.0)
        .px(10.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(theme.color(if selected { "#414141" } else { "#242424" }))
        .hover(theme.color("#4a4a4a"))
        .on_press(move |state: &mut State| state.archive_tab = tab)
        .child(
            text(tab.label())
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(theme.color(if selected { "#f2f2f2" } else { "#c8c8c8" })),
        )
}

fn item_stats(state: &State, entry: &ArchiveEntry) -> String {
    let workspace = if entry.workspace_id.is_empty() {
        "No Workspace"
    } else {
        state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == entry.workspace_id)
            .map_or("Unknown Workspace", |workspace| workspace.name.as_str())
    };
    let unit = if entry.turn_count == 1 {
        "turn"
    } else {
        "turns"
    };
    format!(
        "{workspace} · {} · {} {unit}",
        entry.model, entry.turn_count
    )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let mut tabs = row()
        .key("archive-view-tabs")
        .role("tablist")
        .label("Archive views")
        .gap(6.0);
    for tab in ArchiveTab::ALL {
        tabs = tabs.child(tab_button(tab, state.archive_tab == tab, theme));
    }
    let mut content = vec![tabs];
    if !state.archive_error.is_empty() {
        content.push(
            text(state.archive_error.clone())
                .font(ui_font(13.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let rows: Vec<_> = state
        .archive_entries
        .iter()
        .filter(|entry| entry.quick_chat == (state.archive_tab == ArchiveTab::QuickChats))
        .filter(|entry| !entry.quick_chat || state.quick_chat_id.as_deref() != Some(&entry.id))
        .collect();
    let has_rows = !rows.is_empty();
    let mut list = col().key("archive-list").w_full().mt(6.0);
    if rows.is_empty() {
        list = list.child(
            text(if state.archive_loading && !state.archive_loaded {
                "Loading archive…"
            } else if state.archive_tab == ArchiveTab::Sessions {
                "No archived sessions."
            } else {
                "No archived Quick Chats."
            })
            .font(ui_font(14.0, 400.0))
            .color(theme.color("#969696"))
            .mt(12.0),
        );
    }
    for entry in rows {
        let id = entry.id.clone();
        let quick_chat = entry.quick_chat;
        let busy = state.archive_restore_busy.as_deref() == Some(id.as_str());
        let mut button = form_button(if busy { "Restoring…" } else { "Restore" }, theme)
            .key(format!("archive-restore-{id}"));
        if !state.archive_loading && state.archive_restore_busy.is_none() {
            button = button.on_press(move |state: &mut State| {
                if quick_chat {
                    start_quick_chat_restore(state, id.clone());
                } else {
                    start_archive_restore(state, id.clone());
                }
            });
        }
        list = list
            .child(el().w_full().h(1.0).fill(theme.color("#2c2c2c")))
            .child(
                row()
                    .key(format!("archive-item-{}", entry.id))
                    .w_full()
                    .min_h(62.0)
                    .py(9.0)
                    .center_y()
                    .gap(12.0)
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(4.0)
                            .child(
                                text(entry.title.clone())
                                    .font(ui_font(14.0, 550.0))
                                    .color(theme.color("#e5e5e5"))
                                    .min_w(0.0)
                                    .truncate_fade(),
                            )
                            .child(
                                text(item_stats(state, entry))
                                    .font(ui_font(13.0, 400.0))
                                    .color(theme.color("#969696"))
                                    .min_w(0.0)
                                    .truncate_fade(),
                            ),
                    )
                    .child(button),
            );
    }
    if has_rows {
        list = list.child(el().w_full().h(1.0).fill(theme.color("#2c2c2c")));
    }
    content.push(list);
    content
}
