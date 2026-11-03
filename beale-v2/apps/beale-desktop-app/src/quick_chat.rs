use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, QUICK_CHAT_TURNS_PATH,
    v1::{QuickChatDetail, QuickChatTurnRequest, SessionRole},
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{Color, Cx, El, ScrollSpec, TextEdit, Transition, col, el, icon, row, text, text_input};
use std::{net::SocketAddr, time::Duration};

use crate::{
    AUTH_HANDLE, ConnectionStatus, SelectionMenu, State, ThemeStyle,
    icons::{ARROW_UP, CHEVRON_DOWN, CHEVRON_UP, CLOSE, FOLDER_OPEN},
    new_research::transcript_message,
    provider_auth, toggle_selection_menu, ui_font,
};

const PANEL_WIDTH: f32 = 390.0;
const HEADER_HEIGHT: f32 = 39.0;
const COMPOSER_HEIGHT: f32 = 54.0;

pub(crate) fn valid_detail(detail: &QuickChatDetail, expected_workspace_id: &str) -> bool {
    uuid::Uuid::parse_str(&detail.chat_id).is_ok()
        && detail.workspace_id == expected_workspace_id
        && !detail.title.trim().is_empty()
        && detail.title.chars().count() <= 72
        && !detail.messages.is_empty()
        && detail.messages.len() <= 64
        && detail.messages.len().is_multiple_of(2)
        && detail.messages.iter().enumerate().all(|(index, message)| {
            !message.text.trim().is_empty()
                && message.text.len() <= 65_536
                && message.role
                    == if index % 2 == 0 {
                        SessionRole::User as i32
                    } else {
                        SessionRole::Assistant as i32
                    }
        })
        && detail
            .messages
            .iter()
            .map(|message| message.text.len())
            .sum::<usize>()
            <= 131_072
}

async fn submit(
    addr: SocketAddr,
    request: QuickChatTurnRequest,
) -> Result<QuickChatDetail, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(150))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize Quick Chat connection".to_owned())?;
    let token = provider_auth::authenticated_token(&client, addr).await?;
    let response = client
        .post(format!("http://{addr}{QUICK_CHAT_TURNS_PATH}"))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(request.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not reach the app-server".to_owned())?;
    if response.status() != StatusCode::OK {
        return Err(match response.status() {
            StatusCode::CONFLICT => {
                "Configure a provider, or start a new Quick Chat after this chat reaches its limit."
                    .to_owned()
            }
            StatusCode::BAD_GATEWAY => {
                "The provider could not complete the Quick Chat message.".to_owned()
            }
            StatusCode::PAYLOAD_TOO_LARGE => {
                "The selected workspace has too much context for Quick Chat.".to_owned()
            }
            StatusCode::NOT_FOUND => {
                "The selected workspace or Quick Chat is unavailable.".to_owned()
            }
            _ => "Quick Chat could not send the message.".to_owned(),
        });
    }
    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err("Invalid Quick Chat response".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read Quick Chat response".to_owned())?;
    if bytes.len() > 150_000 {
        return Err("Quick Chat response is too large".to_owned());
    }
    let detail =
        QuickChatDetail::decode(bytes).map_err(|_| "Invalid Quick Chat response".to_owned())?;
    if !valid_detail(&detail, &request.workspace_id) {
        return Err("Invalid Quick Chat response".to_owned());
    }
    Ok(detail)
}

pub(crate) fn send(state: &mut State) {
    if !state.quick_chat_open || state.quick_chat_busy {
        return;
    }
    let prompt = state.quick_chat_draft.text.trim().to_owned();
    if prompt.is_empty() {
        return;
    }
    if prompt.len() > 4_096 {
        state.quick_chat_error = "Quick Chat messages must be under 4 KB.".to_owned();
        return;
    }
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        state.quick_chat_error = "App-server connection is not ready.".to_owned();
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.quick_chat_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    let chat_id = state.quick_chat_id.clone().unwrap_or_default();
    let workspace_id = state.quick_chat_workspace_id.clone().unwrap_or_default();
    if !workspace_id.is_empty()
        && !state
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
    {
        state.quick_chat_error = "The selected workspace is unavailable.".to_owned();
        return;
    }
    let request = QuickChatTurnRequest {
        chat_id,
        prompt: prompt.clone(),
        workspace_id,
    };
    if state.quick_chat_id.is_none() {
        state.quick_chat_title = prompt.chars().take(72).collect();
    }
    if state.selection_menu == Some(SelectionMenu::QuickChatWorkspace) {
        state.selection_menu = None;
    }
    state.quick_chat_generation = state.quick_chat_generation.wrapping_add(1);
    let generation = state.quick_chat_generation;
    state.quick_chat_busy = true;
    state.quick_chat_error.clear();
    state.quick_chat_pending = Some(prompt.clone());
    state.quick_chat_draft.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Quick Chat connection".to_owned())?;
            runtime.block_on(submit(addr, request))
        })();
        handle.update(move |state, _| {
            if state.quick_chat_generation != generation {
                return;
            }
            state.quick_chat_busy = false;
            state.quick_chat_pending = None;
            match result {
                Ok(detail) => {
                    state.quick_chat_id = Some(detail.chat_id);
                    state.quick_chat_title = detail.title;
                    state.quick_chat_messages = detail.messages;
                    state.quick_chat_error.clear();
                    if state.primary_view == crate::PrimaryView::Settings
                        && state.settings_section == crate::SettingsSection::Archive
                    {
                        crate::start_archive_load(state);
                    }
                }
                Err(error) => {
                    state.quick_chat_error = error;
                    state.quick_chat_draft = TextEdit::new(prompt);
                }
            }
        });
    });
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>, cx: &mut Cx<State>) -> El {
    let palette = theme.palette();
    let ribbon_fill = theme.color(palette.card);
    let composer_fill = theme.color(palette.raised);
    let body_fill = theme.color(palette.chrome);
    let present_count =
        state.quick_chat_messages.len() + usize::from(state.quick_chat_pending.is_some());
    let show_body = present_count > 0 || !state.quick_chat_error.is_empty();
    let window_height = cx.layout_size("app-surface").map_or(720.0, |size| size[1]);
    let body_height = (170.0 + (present_count.saturating_sub(1) as f32 * 32.0))
        .min((window_height - 155.0).max(150.0))
        .min(350.0);
    let started = state.quick_chat_id.is_some() || state.quick_chat_pending.is_some();
    let workspace_menu_active = state.selection_menu == Some(SelectionMenu::QuickChatWorkspace);
    let workspace_name = state
        .quick_chat_workspace_id
        .as_deref()
        .and_then(|id| state.workspaces.iter().find(|workspace| workspace.id == id))
        .map_or("No Workspace", |workspace| workspace.name.as_str());
    let heading = if started {
        text(state.quick_chat_title.clone())
            .key("quick-chat-title")
            .font(ui_font(13.0, 550.0))
            .color(theme.color(palette.text))
            .truncate()
            .min_w(0.0)
    } else {
        row()
            .key("quick-chat-workspace-control")
            .role("button")
            .label(format!("Quick Chat workspace: {workspace_name}"))
            .h(28.0)
            .max_w(280.0)
            .min_w(0.0)
            .px(7.0)
            .center_y()
            .gap(7.0)
            .radius(8.0)
            .continuous_corners()
            .fill(if workspace_menu_active {
                theme.color(palette.border)
            } else {
                Color::TRANSPARENT
            })
            .hover(theme.color(palette.border))
            .on_press(|state: &mut State| {
                toggle_selection_menu(state, SelectionMenu::QuickChatWorkspace)
            })
            .child(icon(&FOLDER_OPEN, 15.0).color(theme.color(palette.muted)))
            .child(
                text(workspace_name)
                    .font(ui_font(13.0, 500.0))
                    .color(theme.color(palette.text))
                    .truncate()
                    .min_w(0.0),
            )
            .child(
                icon(
                    if workspace_menu_active {
                        &CHEVRON_UP
                    } else {
                        &CHEVRON_DOWN
                    },
                    13.0,
                )
                .color(theme.color(palette.muted)),
            )
    };
    let header = row()
        .key("quick-chat-ribbon")
        .w_full()
        .h(HEADER_HEIGHT)
        .px(13.0)
        .center_y()
        .gap(8.0)
        .corners([17.0, 17.0, 0.0, 0.0])
        .continuous_corners()
        .fill(ribbon_fill)
        .child(heading)
        .child(el().grow(1.0))
        .child(
            el().key("quick-chat-close")
                .role("button")
                .label("Close Quick Chat")
                .size(27.0, 27.0)
                .radius(8.0)
                .center()
                .hover(theme.color(palette.border))
                .on_press(State::close_quick_chat)
                .child(icon(&CLOSE, 15.0).color(theme.color(palette.muted))),
        );
    let mut panel = col()
        .key("quick-chat-panel")
        .raise()
        .role("dialog")
        .label("Quick Chat")
        .pin(None, None, Some(14.0), Some(14.0))
        .w(PANEL_WIDTH)
        .radius(17.0)
        .continuous_corners()
        .shadow(
            5.0,
            22.0,
            Color::BLACK.alpha(if theme.is_dark() { 0.6 } else { 0.25 }),
        )
        .transition(Transition::asymmetric(
            Transition::offset(PANEL_WIDTH + 24.0, 0.0).combined(Transition::opacity()),
            Transition::offset(PANEL_WIDTH + 24.0, 0.0).combined(Transition::opacity()),
        ))
        .child(col().w_full().px(20.0).child(header));
    if show_body {
        let mut content = col()
            .key("quick-chat-content-list")
            .w_full()
            .gap(12.0)
            .px(14.0)
            .py(14.0);
        for (index, message) in state.quick_chat_messages.iter().enumerate() {
            content = content.child(transcript_message(
                format!("quick-chat-message-{index}"),
                message.text.clone(),
                message.role == SessionRole::User as i32,
                false,
                theme,
            ));
        }
        if let Some(prompt) = &state.quick_chat_pending {
            content = content.child(transcript_message(
                "quick-chat-pending".to_owned(),
                prompt.clone(),
                true,
                false,
                theme,
            ));
        }
        if state.quick_chat_busy {
            content = content.child(
                text("Working…")
                    .key("quick-chat-working")
                    .font(ui_font(12.5, 400.0))
                    .color(theme.color(palette.muted)),
            );
        }
        if !state.quick_chat_error.is_empty() {
            content = content.child(
                text(state.quick_chat_error.clone())
                    .key("quick-chat-error")
                    .font(ui_font(12.5, 400.0))
                    .color(theme.color(palette.error))
                    .wrap(),
            );
        }
        let body = row()
            .key("quick-chat-content")
            .w_full()
            .h(body_height)
            .fill(body_fill)
            .child(
                el().key("quick-chat-content-left-border")
                    .w(1.0)
                    .h_full()
                    .fill(ribbon_fill),
            )
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .h_full()
                    .min_h(0.0)
                    .scroll(ScrollSpec { stick_bottom: true })
                    .child(content),
            )
            .child(
                el().key("quick-chat-content-right-border")
                    .w(1.0)
                    .h_full()
                    .fill(ribbon_fill),
            );
        panel = panel.child(col().w_full().px(20.0).child(body));
    }
    let font = ui_font(13.5, 400.0);
    let composer = row()
        .key("quick-chat-composer")
        .w_full()
        .h(COMPOSER_HEIGHT)
        .px(10.0)
        .center_y()
        .gap(7.0)
        .radius(17.0)
        .continuous_corners()
        .fill(composer_fill)
        .child(
            text_input(&state.quick_chat_draft, font)
                .key("quick-chat-input")
                .role("textbox")
                .label("Quick Chat message")
                .single_line()
                .grow(1.0)
                .min_w(0.0)
                .h(34.0)
                .px(7.0)
                .py(((34.0 - font.line_px()) * 0.5).max(0.0))
                .color(theme.color(palette.text))
                .placeholder("Send a quick message", theme.color(palette.muted))
                .on_edit(|state: &mut State| &mut state.quick_chat_draft)
                .on_submit(|state: &mut State, _| send(state)),
        )
        .child({
            let available =
                !state.quick_chat_busy && !state.quick_chat_draft.text.trim().is_empty();
            let mut button = el()
                .key("quick-chat-send")
                .role("button")
                .label(if available {
                    "Send Quick Chat message"
                } else {
                    "Quick Chat send unavailable"
                })
                .size(28.0, 28.0)
                .center()
                .radius(14.0)
                .fill(theme.color(if available {
                    palette.text
                } else {
                    palette.border
                }))
                .child(icon(&ARROW_UP, 15.0).color(theme.color(if available {
                    palette.panel
                } else {
                    palette.muted
                })));
            if available {
                button = button.hover(theme.color(palette.muted)).on_press(send)
            }
            button
        });
    panel.child(composer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_rejects_malformed_quick_chat_transcripts() {
        let mut detail = QuickChatDetail {
            chat_id: uuid::Uuid::new_v4().to_string(),
            title: "Éxample".into(),
            messages: vec![],
            workspace_id: "workspace-example".into(),
        };
        assert!(!valid_detail(&detail, "workspace-example"));
        detail.messages = vec![
            beale_contracts::v1::QuickChatMessage {
                role: SessionRole::User as i32,
                text: "Example question".into(),
            },
            beale_contracts::v1::QuickChatMessage {
                role: SessionRole::Assistant as i32,
                text: "Example answer".into(),
            },
        ];
        assert!(valid_detail(&detail, "workspace-example"));
        assert!(!valid_detail(&detail, "different-workspace"));
        detail.messages[1].role = SessionRole::User as i32;
        assert!(!valid_detail(&detail, "workspace-example"));
    }
}
