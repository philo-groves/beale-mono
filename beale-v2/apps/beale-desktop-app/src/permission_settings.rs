use beale_contracts::{
    PERMISSION_RULES_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{
        CreatePermissionRuleRequest, PermissionRule, PermissionRulesResponse, ReviewStage,
        UpdatePermissionRuleRequest,
    },
};
use prost::Message;
use reqwest::Client;
use rune::{El, TextEdit, col, el, row, text, text_input};
use std::net::SocketAddr;

use crate::{
    AUTH_HANDLE, ConnectionStatus, State, ThemeStyle, connection, form_button,
    form_controls::{icon_action_button, toggle_button},
    icons::{PEN, PLUS},
    modal_dialog, settings_form_card, ui_font,
    workspace_creation::{field, row_with_control},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PermissionTab {
    AutoReview,
    Guardian,
}

impl PermissionTab {
    const ALL: [Self; 2] = [Self::AutoReview, Self::Guardian];
    fn label(self) -> &'static str {
        match self {
            Self::AutoReview => "Auto-Review",
            Self::Guardian => "Guardian",
        }
    }
    fn stage(self) -> ReviewStage {
        match self {
            Self::AutoReview => ReviewStage::AutoReview,
            Self::Guardian => ReviewStage::Guardian,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::AutoReview => "safety-auto-review",
            Self::Guardian => "safety-guardian",
        }
    }
    fn add_key(self) -> &'static str {
        match self {
            Self::AutoReview => "permission-add-auto-review",
            Self::Guardian => "permission-add-guardian",
        }
    }
}

enum RuleWrite {
    Create(CreatePermissionRuleRequest),
    Update(UpdatePermissionRuleRequest),
}

async fn decode<T: Message + Default>(response: reqwest::Response) -> Result<T, String> {
    if !response.status().is_success() {
        return Err(format!("Safety request failed ({})", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read Safety response".to_owned())?;
    if bytes.len() > 40_000 {
        return Err("Safety response is too large".to_owned());
    }
    T::decode(bytes).map_err(|_| "Invalid Safety response".to_owned())
}

async fn load(client: &Client, addr: SocketAddr) -> Result<Vec<PermissionRule>, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let rules: PermissionRulesResponse = decode(
        client
            .get(format!("http://{addr}{PERMISSION_RULES_PATH}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Could not load permission rules".to_owned())?,
    )
    .await?;
    if rules.rules.len() > 16
        || rules.rules.iter().any(|rule| {
            rule.id.is_empty()
                || rule.title.is_empty()
                || rule.title.len() > 120
                || rule.instruction.len() > 2_000
                || ReviewStage::try_from(rule.stage).is_err()
        })
    {
        return Err("Invalid permission rule catalog".to_owned());
    }
    Ok(rules.rules)
}

async fn write(
    client: &Client,
    addr: SocketAddr,
    request: RuleWrite,
) -> Result<PermissionRule, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let url = format!("http://{addr}{PERMISSION_RULES_PATH}");
    let request = match request {
        RuleWrite::Create(request) => client.post(url).body(request.encode_to_vec()),
        RuleWrite::Update(request) => client.put(url).body(request.encode_to_vec()),
    };
    decode(
        request
            .bearer_auth(token)
            .header("content-type", PROTOBUF_CONTENT_TYPE)
            .send()
            .await
            .map_err(|_| "Could not save permission rule".to_owned())?,
    )
    .await
}

pub(crate) fn start_load(state: &mut State) {
    if state.permission_loading || !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.permission_loading = true;
    state.permission_error.clear();
    state.permission_generation = state.permission_generation.wrapping_add(1);
    let generation = state.permission_generation;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(load(&client, addr))
        })();
        handle.update(move |state, _| {
            if state.permission_generation != generation {
                return;
            }
            state.permission_loading = false;
            match result {
                Ok(rules) => {
                    state.permission_rules = rules;
                    state.permission_loaded = true;
                }
                Err(error) => state.permission_error = error,
            }
        });
    });
}

fn start_save(state: &mut State, id: String, title: String, instruction: String, enabled: bool) {
    start_write(
        state,
        RuleWrite::Update(UpdatePermissionRuleRequest {
            id,
            title,
            instruction,
            enabled,
        }),
    );
}

fn start_create(state: &mut State, title: String, instruction: String) {
    let stage = state.permissions_tab.stage() as i32;
    start_write(
        state,
        RuleWrite::Create(CreatePermissionRuleRequest {
            stage,
            title,
            instruction,
        }),
    );
}

fn start_write(state: &mut State, request: RuleWrite) {
    if state.permission_saving.is_some() {
        return;
    }
    if !matches!(state.connection, ConnectionStatus::Connected { .. }) {
        if state.permission_edit_id.is_some() || state.permission_creating {
            state.permission_edit_error = "App-server is not connected.".to_owned();
        } else {
            state.permission_error = "App-server is not connected.".to_owned();
        }
        return;
    }
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        if state.permission_edit_id.is_some() || state.permission_creating {
            state.permission_edit_error = "App-server is not ready.".to_owned();
        } else {
            state.permission_error = "App-server is not ready.".to_owned();
        }
        return;
    };
    state.permission_saving = Some(match &request {
        RuleWrite::Create(_) => "new".to_owned(),
        RuleWrite::Update(request) => request.id.clone(),
    });
    state.permission_error.clear();
    state.permission_edit_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize connection".to_owned())?;
            runtime.block_on(write(&client, addr, request))
        })();
        handle.update(move |state, _| {
            state.permission_saving = None;
            match result {
                Ok(rule) => {
                    if let Some(existing) = state
                        .permission_rules
                        .iter_mut()
                        .find(|item| item.id == rule.id)
                    {
                        *existing = rule;
                    } else {
                        state.permission_rules.push(rule);
                    }
                    state.permission_edit_id = None;
                    state.permission_creating = false;
                    state.permission_name_edit.clear();
                    state.permission_description_edit.clear();
                    state.permission_edit_error.clear();
                }
                Err(error) => {
                    if state.permission_edit_id.is_some() || state.permission_creating {
                        state.permission_edit_error = error;
                    } else {
                        state.permission_error = error;
                    }
                }
            }
        });
    });
}

fn tab_button(tab: PermissionTab, selected: bool, theme: ThemeStyle<'_>) -> El {
    row()
        .key(tab.key())
        .role("tab")
        .label(tab.label())
        .h(26.0)
        .px(10.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(theme.color(if selected { "#414141" } else { "#242424" }))
        .hover(theme.color("#4a4a4a"))
        .on_press(move |state: &mut State| {
            state.navigate_to(crate::ViewLocation::Safety(tab));
        })
        .child(
            text(tab.label())
                .font(ui_font(14.0, if selected { 600.0 } else { 450.0 }))
                .color(theme.color(if selected { "#f2f2f2" } else { "#c8c8c8" })),
        )
}

fn rule_row(state: &State, rule: &PermissionRule, theme: ThemeStyle<'_>) -> El {
    let saving = state.permission_saving.is_some();
    let id = rule.id.clone();
    let enabled = rule.enabled;
    let edit_id = rule.id.clone();
    let detail = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(5.0)
        .child(
            text(rule.title.clone())
                .font(ui_font(14.0, 550.0))
                .color(theme.color("#e2e2e2")),
        )
        .child(
            text(rule.instruction.clone())
                .w_full()
                .wrap()
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    let mut body = row().w_full().gap(8.0).center_y().child(detail);
    let mut edit = icon_action_button(
        format!("permission-edit-{edit_id}"),
        format!("Edit {}", rule.title),
        &PEN,
        theme,
    );
    if !saving {
        edit = edit.on_press(move |state: &mut State| {
            if let Some(rule) = state
                .permission_rules
                .iter()
                .find(|rule| rule.id == edit_id)
            {
                state.permission_name_edit = TextEdit::new(rule.title.clone());
                state.permission_description_edit = TextEdit::new(rule.instruction.clone());
                state.permission_edit_error.clear();
                state.permission_edit_id = Some(edit_id.clone());
            }
        });
    }
    body = body.child(edit);
    let mut toggle = toggle_button(
        format!("permission-toggle-{id}"),
        &rule.title,
        enabled,
        !saving,
        theme,
    );
    if !saving {
        toggle = toggle.on_press(move |state: &mut State| {
            if let Some(rule) = state.permission_rules.iter().find(|rule| rule.id == id) {
                start_save(
                    state,
                    id.clone(),
                    rule.title.clone(),
                    rule.instruction.clone(),
                    !enabled,
                );
            }
        });
    }
    body = body.child(toggle);
    col().w_full().py(12.0).gap(8.0).child(body)
}

fn close_edit_dialog(state: &mut State) {
    if state.permission_saving.is_some() {
        return;
    }
    state.permission_edit_id = None;
    state.permission_creating = false;
    state.permission_name_edit.clear();
    state.permission_description_edit.clear();
    state.permission_edit_error.clear();
}

fn save_edit_dialog(state: &mut State) {
    if !state.permission_creating && state.permission_edit_id.is_none() {
        return;
    }
    let title = state.permission_name_edit.text.trim().to_owned();
    let instruction = state.permission_description_edit.text.trim().to_owned();
    if title.is_empty() || title.len() > 120 || title.chars().any(char::is_control) {
        state.permission_edit_error = "Enter a rule name of at most 120 characters.".to_owned();
        return;
    }
    if instruction.is_empty()
        || instruction.len() > 2_000
        || instruction
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        state.permission_edit_error = "Enter a description of at most 2,000 characters.".to_owned();
        return;
    }
    if state.permission_creating {
        start_create(state, title, instruction);
        return;
    }
    let Some(id) = state.permission_edit_id.clone() else {
        return;
    };
    let Some(enabled) = state
        .permission_rules
        .iter()
        .find(|rule| rule.id == id)
        .map(|rule| rule.enabled)
    else {
        return;
    };
    start_save(state, id, title, instruction, enabled);
}

pub(crate) fn edit_dialog(state: &State, theme: ThemeStyle<'_>) -> [El; 2] {
    let creating = state.permission_creating;
    let form = settings_form_card(
        [
            row_with_control(
                "Rule Name",
                "Name shown in Safety.",
                field(
                    &state.permission_name_edit,
                    "permission-name-input",
                    "Rule name",
                    theme,
                    |state| &mut state.permission_name_edit,
                )
                .w(280.0),
                theme,
            ),
            row_with_control(
                "Description",
                "Instruction used by model review.",
                text_input(&state.permission_description_edit, ui_font(14.0, 400.0))
                    .key("permission-description-input")
                    .w(280.0)
                    .h(120.0)
                    .padding(10.0)
                    .radius(8.0)
                    .continuous_corners()
                    .fill(theme.color("#282828"))
                    .color(theme.color("#e2e2e2"))
                    .on_edit(|state: &mut State| &mut state.permission_description_edit),
                theme,
            ),
        ],
        theme,
    );
    let mut body = col().w_full().gap(10.0).child(form);
    if !state.permission_edit_error.is_empty() {
        body = body.child(
            text(state.permission_edit_error.clone())
                .key("permission-edit-error")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let mut save = form_button(
        if state.permission_saving.is_some() {
            "Saving…"
        } else if creating {
            "Add Rule"
        } else {
            "Save"
        },
        theme,
    )
    .key("permission-edit-save");
    if state.permission_saving.is_none() {
        save = save.on_press(save_edit_dialog);
    }
    let actions = row()
        .w_full()
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            form_button("Cancel", theme)
                .key("permission-edit-cancel")
                .on_press(close_edit_dialog),
        )
        .child(save);
    modal_dialog::render(
        modal_dialog::DialogSpec {
            key: "permission-edit-dialog",
            title: if creating {
                match state.permissions_tab {
                    PermissionTab::AutoReview => "Add an Auto-Review Rule",
                    PermissionTab::Guardian => "Add a Guardian Rule",
                }
            } else {
                "Edit Rule"
            },
            width: 640.0,
            focus_return: if creating {
                state.permissions_tab.add_key()
            } else {
                "safety-view-tabs"
            },
        },
        body,
        actions,
        theme,
        close_edit_dialog,
    )
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> Vec<El> {
    let mut tabs = row()
        .key("safety-view-tabs")
        .role("tablist")
        .label("Safety views")
        .gap(6.0);
    for tab in PermissionTab::ALL {
        tabs = tabs.child(tab_button(tab, state.permissions_tab == tab, theme));
    }
    let mut content = vec![tabs];
    let heading = match state.permissions_tab {
        PermissionTab::AutoReview => "Auto-Review Rules",
        PermissionTab::Guardian => "Guardian Rules",
    };
    let description = match state.permissions_tab {
        PermissionTab::AutoReview => {
            "The configured small model reviews read, write, edit, and Bash calls before execution."
        }
        PermissionTab::Guardian => {
            "After an Auto-Review denial, the lead model can review the same call once."
        }
    };
    let tab = state.permissions_tab;
    let mut add = icon_action_button(
        tab.add_key(),
        format!("Add {} Rule", tab.label()),
        &PLUS,
        theme,
    );
    if state.permission_saving.is_none() && state.permission_rules.len() < 16 {
        add = add.on_press(move |state: &mut State| {
            state.permission_creating = true;
            state.permission_edit_id = None;
            state.permission_name_edit.clear();
            state.permission_description_edit.clear();
            state.permission_edit_error.clear();
        });
    }
    let mut form = col()
        .key("permission-rules-form")
        .w_full()
        .gap(12.0)
        .mt(6.0)
        .child(
            col()
                .w_full()
                .gap(4.0)
                .child(
                    row()
                        .w_full()
                        .center_y()
                        .child(
                            text(heading)
                                .key("permission-rules-heading")
                                .grow(1.0)
                                .font(ui_font(17.0, 600.0))
                                .color(theme.color("#f2f2f2")),
                        )
                        .child(add),
                )
                .child(
                    text(description)
                        .key("permission-rules-description")
                        .w_full()
                        .wrap()
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#969696")),
                ),
        );
    if !state.permission_error.is_empty() {
        content.push(
            text(state.permission_error.clone())
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    let rules: Vec<_> = state
        .permission_rules
        .iter()
        .filter(|rule| rule.stage == state.permissions_tab.stage() as i32)
        .collect();
    if rules.is_empty() {
        let label = if state.permission_loading {
            "Loading permission rules…"
        } else if state.permission_loaded {
            "No rules configured."
        } else {
            "Permission rules are unavailable."
        };
        form = form.child(
            text(label)
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
        if !state.permission_loading {
            form = form.child(
                form_button("Retry", theme)
                    .key("permission-retry")
                    .on_press(start_load),
            );
        }
        content.push(form);
        return content;
    }
    content.push(
        form.child(
            settings_form_card(
                rules.into_iter().map(|rule| rule_row(state, rule, theme)),
                theme,
            )
            .key("permission-rules-card"),
        ),
    );
    content
}
