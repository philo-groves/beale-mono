use beale_contracts::{
    CUSTOM_PROVIDER_API_KEY_PATH, CUSTOM_PROVIDER_DISCOVER_PATH, CUSTOM_PROVIDERS_PATH,
    PROTOBUF_CONTENT_TYPE,
    v1::{
        CustomProvider, CustomProviderRequest, DiscoverCustomModelsRequest,
        DiscoverCustomModelsResponse, ListCustomProvidersResponse, SaveCustomProviderRequest,
        SetCustomProviderApiKeyRequest,
    },
};
use prost::Message;
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use rune::{El, TextEdit, col, row, text, text_input};
use std::net::SocketAddr;

use crate::{
    PROVIDER_ERROR_COLOR, State, connection, form_button, form_controls::toggle_button,
    provider_auth::authenticated_token, ui_font,
};

fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

pub(crate) async fn list(client: &Client, addr: SocketAddr) -> Result<Vec<CustomProvider>, String> {
    let token = authenticated_token(client, addr).await?;
    let response = client
        .get(url(addr, CUSTOM_PROVIDERS_PATH))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not load custom providers".to_owned())?;
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(format!(
            "Could not load custom providers ({})",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read custom providers".to_owned())?;
    if bytes.len() > 512_000 {
        return Err("Custom provider list is too large".to_owned());
    }
    Ok(ListCustomProvidersResponse::decode(bytes)
        .map_err(|_| "Invalid custom provider list".to_owned())?
        .providers)
}

pub(crate) async fn discover(
    client: &Client,
    addr: SocketAddr,
    id: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<beale_contracts::v1::ModelEntry>, String> {
    let token = authenticated_token(client, addr).await?;
    let response = client
        .post(url(addr, CUSTOM_PROVIDER_DISCOVER_PATH))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(
            DiscoverCustomModelsRequest {
                id,
                base_url,
                api_key,
            }
            .encode_to_vec(),
        )
        .send()
        .await
        .map_err(|_| "Could not discover custom models".to_owned())?;
    if response.status() != StatusCode::OK {
        return Err(format!(
            "Could not discover custom models ({})",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read custom models".to_owned())?;
    if bytes.len() > 131_072 {
        return Err("Custom model list is too large".to_owned());
    }
    Ok(DiscoverCustomModelsResponse::decode(bytes)
        .map_err(|_| "Invalid custom model list".to_owned())?
        .models)
}

pub(crate) async fn save(
    client: &Client,
    addr: SocketAddr,
    provider: CustomProvider,
    key: Option<String>,
) -> Result<Vec<CustomProvider>, String> {
    let token = authenticated_token(client, addr).await?;
    let response = client
        .post(url(addr, CUSTOM_PROVIDERS_PATH))
        .bearer_auth(&token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(
            SaveCustomProviderRequest {
                provider: Some(provider.clone()),
            }
            .encode_to_vec(),
        )
        .send()
        .await
        .map_err(|_| "Could not save custom provider".to_owned())?;
    if response.status() != StatusCode::NO_CONTENT {
        return Err(format!(
            "Could not save custom provider ({})",
            response.status()
        ));
    }
    if let Some(key) = key {
        let response = client
            .post(url(addr, CUSTOM_PROVIDER_API_KEY_PATH))
            .bearer_auth(token)
            .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
            .body(
                SetCustomProviderApiKeyRequest {
                    id: provider.id,
                    api_key: key,
                }
                .encode_to_vec(),
            )
            .send()
            .await
            .map_err(|_| "Could not save custom provider API key".to_owned())?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(format!(
                "Could not save custom provider API key ({})",
                response.status()
            ));
        }
    }
    list(client, addr).await
}

pub(crate) async fn remove(
    client: &Client,
    addr: SocketAddr,
    id: String,
) -> Result<Vec<CustomProvider>, String> {
    let token = authenticated_token(client, addr).await?;
    let response = client
        .delete(url(addr, CUSTOM_PROVIDERS_PATH))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(CustomProviderRequest { id }.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not remove custom provider".to_owned())?;
    if response.status() != StatusCode::NO_CONTENT {
        return Err(format!(
            "Could not remove custom provider ({})",
            response.status()
        ));
    }
    list(client, addr).await
}

pub(crate) async fn remove_key(
    client: &Client,
    addr: SocketAddr,
    id: String,
) -> Result<Vec<CustomProvider>, String> {
    let token = authenticated_token(client, addr).await?;
    let response = client
        .delete(url(addr, CUSTOM_PROVIDER_API_KEY_PATH))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(CustomProviderRequest { id }.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not remove custom provider API key".to_owned())?;
    if response.status() != StatusCode::NO_CONTENT {
        return Err(format!(
            "Could not remove custom provider API key ({})",
            response.status()
        ));
    }
    list(client, addr).await
}

fn clear_editor(state: &mut State) {
    state.custom_editor_open = false;
    state.custom_edit_id = None;
    state.custom_name_edit.clear();
    state.custom_url_edit.clear();
    state.custom_models_edit.clear();
    state.custom_discovered_models.clear();
    state.custom_lead_edit.clear();
    state.custom_small_edit.clear();
    state.custom_key_edit.clear();
    state.custom_reasoning = false;
    state.custom_error.clear();
}

fn edit_provider(state: &mut State, id: &str) {
    let Some(provider) = state
        .custom_providers
        .iter()
        .find(|provider| provider.id == id)
    else {
        return;
    };
    state.custom_editor_open = true;
    state.custom_edit_id = Some(id.to_owned());
    state.custom_name_edit = TextEdit::new(&provider.name);
    state.custom_url_edit = TextEdit::new(&provider.base_url);
    state.custom_models_edit = TextEdit::new(
        provider
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    );
    state.custom_discovered_models = provider.models.clone();
    state.custom_lead_edit = TextEdit::new(&provider.lead_model);
    state.custom_small_edit = TextEdit::new(&provider.small_model);
    state.custom_key_edit.clear();
    state.custom_reasoning = provider.reasoning;
    state.custom_error.clear();
}

fn start_save(state: &mut State) {
    if state.custom_busy {
        return;
    }
    let name = state.custom_name_edit.text.trim().to_owned();
    let base_url = state.custom_url_edit.text.trim().to_owned();
    let models: Vec<_> = state
        .custom_models_edit
        .text
        .split([',', '\n'])
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| beale_contracts::v1::ModelEntry {
            id: id.to_owned(),
            display_name: state
                .custom_discovered_models
                .iter()
                .find(|model| model.id == id)
                .map_or_else(|| id.replace('-', " "), |model| model.display_name.clone()),
        })
        .collect();
    if name.is_empty() || base_url.is_empty() || models.is_empty() {
        state.custom_error = "Enter a name, API base URL, and at least one model ID.".to_owned();
        return;
    }
    if let Some(existing) = state.custom_edit_id.as_ref().and_then(|id| {
        state
            .custom_providers
            .iter()
            .find(|provider| &provider.id == id)
    }) && existing.api_key_configured
        && existing.base_url != base_url.trim_end_matches('/')
    {
        state.custom_error = "Forget the saved key before changing this API URL.".to_owned();
        return;
    }
    let lead_model = state.custom_lead_edit.text.trim();
    let small_model = state.custom_small_edit.text.trim();
    let lead_model = if lead_model.is_empty() {
        models[0].id.clone()
    } else {
        lead_model.to_owned()
    };
    let small_model = if small_model.is_empty() {
        lead_model.clone()
    } else {
        small_model.to_owned()
    };
    if !models.iter().any(|model| model.id == lead_model)
        || !models.iter().any(|model| model.id == small_model)
    {
        state.custom_error = "Default models must be included in the model IDs.".to_owned();
        return;
    }
    let Some(handle) = crate::AUTH_HANDLE.get().cloned() else {
        state.custom_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    let provider = CustomProvider {
        id: state
            .custom_edit_id
            .clone()
            .unwrap_or_else(|| format!("custom-{}", uuid::Uuid::new_v4().simple())),
        name,
        base_url,
        models,
        lead_model,
        small_model,
        reasoning: state.custom_reasoning,
        api_key_configured: false,
    };
    let key = state.custom_key_edit.text.trim().to_owned();
    let key = (!key.is_empty()).then_some(key);
    state.custom_busy = true;
    state.custom_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            runtime.block_on(save(&client, addr, provider, key))
        })();
        handle.update(move |state, _| {
            state.custom_busy = false;
            match result {
                Ok(providers) => {
                    state.custom_providers = providers;
                    clear_editor(state);
                }
                Err(error) => state.custom_error = error,
            }
        });
    });
}

fn start_discover(state: &mut State) {
    if state.custom_busy {
        return;
    }
    let base_url = state.custom_url_edit.text.trim().to_owned();
    if base_url.is_empty() {
        state.custom_error = "Enter the API base URL first.".to_owned();
        return;
    }
    let Some(handle) = crate::AUTH_HANDLE.get().cloned() else {
        state.custom_error = "Desktop connection is not ready.".to_owned();
        return;
    };
    let id = state.custom_edit_id.clone().unwrap_or_default();
    let api_key = state.custom_key_edit.text.trim().to_owned();
    state.custom_busy = true;
    state.custom_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            runtime.block_on(discover(&client, addr, id, base_url, api_key))
        })();
        handle.update(move |state, _| {
            state.custom_busy = false;
            match result {
                Ok(models) => {
                    state.custom_models_edit = TextEdit::new(
                        models
                            .iter()
                            .map(|model| model.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    state.custom_discovered_models = models;
                    if !state
                        .custom_discovered_models
                        .iter()
                        .any(|model| model.id == state.custom_lead_edit.text)
                    {
                        state.custom_lead_edit.clear();
                    }
                    if !state
                        .custom_discovered_models
                        .iter()
                        .any(|model| model.id == state.custom_small_edit.text)
                    {
                        state.custom_small_edit.clear();
                    }
                }
                Err(error) => state.custom_error = error,
            }
        });
    });
}

fn start_remove(state: &mut State, id: String) {
    if state.custom_busy {
        return;
    }
    let Some(handle) = crate::AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.custom_busy = true;
    state.custom_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            runtime.block_on(remove(&client, addr, id))
        })();
        handle.update(move |state, _| {
            state.custom_busy = false;
            state.custom_delete_id = None;
            match result {
                Ok(providers) => {
                    state.custom_providers = providers;
                    clear_editor(state);
                }
                Err(error) => state.custom_error = error,
            }
        });
    });
}

fn start_remove_key(state: &mut State, id: String) {
    if state.custom_busy {
        return;
    }
    let Some(handle) = crate::AUTH_HANDLE.get().cloned() else {
        return;
    };
    state.custom_busy = true;
    state.custom_error.clear();
    std::thread::spawn(move || {
        let result = (|| {
            let addr = connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = connection::http_client()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize app-server connection".to_owned())?;
            runtime.block_on(remove_key(&client, addr, id))
        })();
        handle.update(move |state, _| {
            state.custom_busy = false;
            match result {
                Ok(providers) => state.custom_providers = providers,
                Err(error) => state.custom_error = error,
            }
        });
    });
}

fn field(
    state: &State,
    label: &'static str,
    key: &'static str,
    edit: &TextEdit,
    secure: bool,
    accessor: fn(&mut State) -> &mut TextEdit,
) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let mut input = text_input(edit, ui_font(14.5, 400.0))
        .key(key)
        .single_line()
        .w_full()
        .h(36.0)
        .padding(8.0)
        .radius(8.0)
        .continuous_corners()
        .fill(color("#242424"))
        .color(color("#e2e2e2"))
        .on_edit(accessor);
    if secure {
        input = input.secure();
    }
    col()
        .w_full()
        .gap(6.0)
        .child(
            text(label)
                .font(ui_font(14.0, 500.0))
                .color(color("#d4d4d4")),
        )
        .child(input)
}

pub(crate) fn render(state: &State) -> El {
    let theme = state.appearance.style();
    let color = |value| theme.color(value);
    let mut detail = col().key("custom-provider-detail").role("tabpanel").label("Custom API providers")
        .w_full().gap(20.0)
        .child(col().w_full().gap(7.0).padding(14.0).radius(16.0).continuous_corners().border(1.0, color("#303030"))
            .child(text("Custom API Providers").font(ui_font(23.0, 600.0)).color(color("#f2f2f2")))
            .child(text("Connect OpenAI-compatible chat completions APIs. Model IDs and API keys stay on this host.")
                .w_full().wrap().font(ui_font(14.5, 400.0)).color(color("#969696"))))
        .child(row().w_full().center_y()
            .child(text("Providers").grow(1.0).font(ui_font(17.0, 600.0)).color(color("#f2f2f2")))
            .child(form_button("Add Provider", theme).on_press(|s: &mut State| { clear_editor(s); s.custom_editor_open = true; })));
    for provider in &state.custom_providers {
        let id = provider.id.clone();
        let name = provider.name.clone();
        let model_count = provider.models.len();
        detail =
            detail.child(
                col()
                    .key(format!("custom-provider-{id}"))
                    .w_full()
                    .gap(10.0)
                    .padding(13.0)
                    .radius(12.0)
                    .border(1.0, color("#303030"))
                    .child(
                        row()
                            .w_full()
                            .center_y()
                            .gap(10.0)
                            .child(
                                text(name)
                                    .grow(1.0)
                                    .font(ui_font(15.0, 550.0))
                                    .color(color("#e2e2e2")),
                            )
                            .child(
                                text(if provider.api_key_configured {
                                    "Configured"
                                } else {
                                    "No API Key"
                                })
                                .font(ui_font(13.5, 400.0))
                                .color(color(
                                    if provider.api_key_configured {
                                        "#8cbe98"
                                    } else {
                                        "#969696"
                                    },
                                )),
                            )
                            .child(form_button("Edit", theme).on_press({
                                let id = id.clone();
                                move |s: &mut State| edit_provider(s, &id)
                            }))
                            .child(if provider.api_key_configured {
                                form_button("Forget Key", theme).on_press({
                                    let id = id.clone();
                                    move |s: &mut State| start_remove_key(s, id.clone())
                                })
                            } else {
                                rune::el().w(0.0)
                            })
                            .child(form_button("Remove", theme).on_press(move |s: &mut State| {
                                s.custom_delete_id = Some(id.clone())
                            })),
                    )
                    .child(
                        text(format!(
                            "{} · {model_count} model{}",
                            provider.base_url,
                            if model_count == 1 { "" } else { "s" }
                        ))
                        .w_full()
                        .wrap()
                        .font(ui_font(13.5, 400.0))
                        .color(color("#969696")),
                    ),
            );
    }
    if let Some(id) = &state.custom_delete_id
        && let Some(provider) = state
            .custom_providers
            .iter()
            .find(|provider| provider.id == *id)
    {
        let id = id.clone();
        detail = detail.child(
            col()
                .w_full()
                .gap(10.0)
                .padding(13.0)
                .radius(12.0)
                .fill(color("#242424"))
                .child(
                    text(format!("Remove {} and its saved API key?", provider.name))
                        .font(ui_font(14.0, 500.0))
                        .color(color("#e2e2e2")),
                )
                .child(
                    row()
                        .gap(8.0)
                        .child(
                            form_button("Remove", theme)
                                .on_press(move |s: &mut State| start_remove(s, id.clone())),
                        )
                        .child(
                            form_button("Cancel", theme)
                                .on_press(|s: &mut State| s.custom_delete_id = None),
                        ),
                ),
        );
    }
    if state.custom_editor_open {
        detail = detail.child(
            col()
                .key("custom-provider-editor")
                .w_full()
                .gap(12.0)
                .padding(16.0)
                .radius(12.0)
                .fill(color("#191919"))
                .child(
                    text(if state.custom_edit_id.is_some() {
                        "Edit Provider"
                    } else {
                        "Add Provider"
                    })
                    .font(ui_font(17.0, 600.0))
                    .color(color("#f2f2f2")),
                )
                .child(field(
                    state,
                    "Name",
                    "custom-name",
                    &state.custom_name_edit,
                    false,
                    |s| &mut s.custom_name_edit,
                ))
                .child(field(
                    state,
                    "API Base URL",
                    "custom-url",
                    &state.custom_url_edit,
                    false,
                    |s| &mut s.custom_url_edit,
                ))
                .child(field(
                    state,
                    "Model IDs (comma separated)",
                    "custom-models",
                    &state.custom_models_edit,
                    false,
                    |s| &mut s.custom_models_edit,
                ))
                .child(
                    form_button(
                        if state.custom_busy {
                            "Discovering"
                        } else {
                            "Discover Models"
                        },
                        theme,
                    )
                    .on_press(start_discover),
                )
                .child(field(
                    state,
                    "Lead Model ID (defaults to first)",
                    "custom-lead",
                    &state.custom_lead_edit,
                    false,
                    |s| &mut s.custom_lead_edit,
                ))
                .child(field(
                    state,
                    "Small Model ID (defaults to lead)",
                    "custom-small",
                    &state.custom_small_edit,
                    false,
                    |s| &mut s.custom_small_edit,
                ))
                .child(
                    row()
                        .w_full()
                        .center_y()
                        .child(
                            text("OpenAI-compatible reasoning effort")
                                .grow(1.0)
                                .font(ui_font(14.0, 500.0))
                                .color(color("#d4d4d4")),
                        )
                        .child(
                            toggle_button(
                                "custom-reasoning",
                                "Reasoning effort",
                                state.custom_reasoning,
                                true,
                                theme,
                            )
                            .on_press(|state: &mut State| {
                                state.custom_reasoning = !state.custom_reasoning
                            }),
                        ),
                )
                .child(field(
                    state,
                    "API Key (leave blank to keep saved key)",
                    "custom-key",
                    &state.custom_key_edit,
                    true,
                    |s| &mut s.custom_key_edit,
                ))
                .child(
                    row()
                        .gap(8.0)
                        .child(
                            form_button(if state.custom_busy { "Saving" } else { "Save" }, theme)
                                .on_press(start_save),
                        )
                        .child(form_button("Cancel", theme).on_press(clear_editor)),
                ),
        );
    }
    if !state.custom_error.is_empty() {
        detail = detail.child(
            text(state.custom_error.clone())
                .font(ui_font(14.0, 400.0))
                .color(color(PROVIDER_ERROR_COLOR)),
        );
    }
    detail
}
