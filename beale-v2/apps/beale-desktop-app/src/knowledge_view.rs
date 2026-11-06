use beale_contracts::knowledge::{
    self, Data, GetRequest, GetResponse, Kind, QueryRequest, QueryResponse, Record, Summary,
};
use reqwest::{Client, StatusCode};
use rune::{El, TextEdit, col, el, row, text, text_input};
use std::net::SocketAddr;

use crate::{AUTH_HANDLE, State, ThemeStyle, form_button, ui_font};

pub(crate) struct KnowledgeViewState {
    pub(crate) query: TextEdit,
    pub(crate) kind: Option<Kind>,
    pub(crate) records: Vec<Summary>,
    pub(crate) selected: Option<Record>,
    pub(crate) selected_id: Option<String>,
    pub(crate) loading: bool,
    pub(crate) detail_loading: bool,
    pub(crate) next_history_offset: Option<u32>,
    pub(crate) error: String,
    pub(crate) generation: u64,
}

impl Default for KnowledgeViewState {
    fn default() -> Self {
        Self {
            query: TextEdit::new(""),
            kind: None,
            records: Vec::new(),
            selected: None,
            selected_id: None,
            loading: false,
            detail_loading: false,
            next_history_offset: None,
            error: String::new(),
            generation: 0,
        }
    }
}

async fn load(
    client: &Client,
    addr: SocketAddr,
    input: QueryRequest,
) -> Result<QueryResponse, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(format!("http://{addr}{}", knowledge::QUERY_PATH))
        .bearer_auth(token)
        .json(&input)
        .send()
        .await
        .map_err(|_| "Could not load Knowledge Base".to_owned())?;
    if response.status() != StatusCode::OK {
        return Err(format!(
            "Knowledge Base unavailable ({})",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read Knowledge Base".to_owned())?;
    if bytes.len() > 512_000 {
        return Err("Knowledge catalog is too large".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid Knowledge catalog".to_owned())
}

async fn load_one(
    client: &Client,
    addr: SocketAddr,
    input: GetRequest,
) -> Result<GetResponse, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(format!("http://{addr}{}", knowledge::GET_PATH))
        .bearer_auth(token)
        .json(&input)
        .send()
        .await
        .map_err(|_| "Could not load Knowledge record".to_owned())?;
    if response.status() != StatusCode::OK {
        return Err(format!(
            "Knowledge record unavailable ({})",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "Could not read Knowledge record".to_owned())?;
    if bytes.len() > 256_000 {
        return Err("Knowledge record is too large".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid Knowledge record".to_owned())
}

pub(crate) fn start_load(state: &mut State) {
    let Some(workspace_id) = state.workspace_id.map(|id| id.to_string()) else {
        return;
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.knowledge.error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.knowledge.generation = state.knowledge.generation.wrapping_add(1);
    let generation = state.knowledge.generation;
    state.knowledge.loading = true;
    state.knowledge.error.clear();
    state.knowledge.selected = None;
    state.knowledge.selected_id = None;
    state.knowledge.next_history_offset = None;
    let input = QueryRequest {
        workspace_id: workspace_id.clone(),
        text: state.knowledge.query.text.clone(),
        kind: state.knowledge.kind,
        limit: 50,
        ..Default::default()
    };
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = crate::workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Knowledge connection".to_owned())?;
            runtime.block_on(load(&client, addr, input))
        })();
        handle.update(move |state, _| {
            if state.workspace_id.map(|id| id.to_string()).as_deref() != Some(&workspace_id)
                || state.knowledge.generation != generation
            {
                return;
            }
            state.knowledge.loading = false;
            match result {
                Ok(response) => {
                    state.knowledge.records = response.records;
                    state.knowledge.error.clear();
                }
                Err(error) => state.knowledge.error = error,
            }
        });
    });
}

fn select(state: &mut State, id: String) {
    select_page(state, id, 0);
}

fn select_page(state: &mut State, id: String, history_offset: u32) {
    let Some(workspace_id) = state.workspace_id.map(|value| value.to_string()) else {
        return;
    };
    let Some(handle) = AUTH_HANDLE.get().cloned() else {
        state.knowledge.error = "Desktop connection is not ready.".to_owned();
        return;
    };
    state.knowledge.selected_id = Some(id.clone());
    if history_offset == 0 {
        state.knowledge.selected = None;
        state.knowledge.next_history_offset = None;
    }
    state.knowledge.detail_loading = true;
    state.knowledge.error.clear();
    let generation = state.knowledge.generation;
    std::thread::spawn(move || {
        let result = (|| {
            let addr = crate::connection::configured_addr()
                .map_err(|_| "Invalid local app-server address".to_owned())?;
            let client = crate::workspaces::client()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Could not initialize Knowledge connection".to_owned())?;
            runtime.block_on(load_one(
                &client,
                addr,
                GetRequest {
                    workspace_id: workspace_id.clone(),
                    id: id.clone(),
                    history_offset,
                    history_limit: 5,
                },
            ))
        })();
        handle.update(move |state, _| {
            if state.workspace_id.map(|value| value.to_string()).as_deref() != Some(&workspace_id)
                || state.knowledge.generation != generation
                || state.knowledge.selected_id.as_deref() != Some(&id)
            {
                return;
            }
            state.knowledge.detail_loading = false;
            match result {
                Ok(response) => {
                    state.knowledge.next_history_offset = response.next_history_offset;
                    if history_offset > 0
                        && let Some(current) = &mut state.knowledge.selected
                        && current.revision == response.record.revision
                    {
                        current.history.extend(response.record.history);
                    } else {
                        state.knowledge.selected = Some(response.record);
                    }
                    state.knowledge.error.clear();
                }
                Err(error) => state.knowledge.error = error,
            }
        });
    });
}

fn filter_button(
    label: &'static str,
    kind: Option<Kind>,
    selected: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let button = form_button(label, theme).key(format!("knowledge-filter-{label}"));
    if selected {
        button
    } else {
        button.on_press(move |state: &mut State| {
            state.knowledge.kind = kind;
            start_load(state);
        })
    }
}

pub(crate) fn render(state: &State, theme: ThemeStyle<'_>) -> El {
    let mut page = col()
        .key("knowledge-page")
        .w_full()
        .gap(14.0)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .child(filter_button(
                    "All",
                    None,
                    state.knowledge.kind.is_none(),
                    theme,
                ))
                .child(filter_button(
                    "Findings",
                    Some(Kind::Claim),
                    state.knowledge.kind == Some(Kind::Claim),
                    theme,
                ))
                .child(filter_button(
                    "Memories",
                    Some(Kind::Memory),
                    state.knowledge.kind == Some(Kind::Memory),
                    theme,
                )),
        )
        .child(
            row()
                .w_full()
                .gap(8.0)
                .child(
                    text_input(&state.knowledge.query, ui_font(14.0, 400.0))
                        .key("knowledge-search-input")
                        .grow(1.0)
                        .h(38.0)
                        .padding(9.0)
                        .radius(8.0)
                        .fill(theme.color("#282828"))
                        .color(theme.color("#e2e2e2"))
                        .on_edit(|state: &mut State| &mut state.knowledge.query),
                )
                .child(
                    form_button("Search", theme)
                        .key("knowledge-search")
                        .on_press(start_load),
                ),
        );
    if state.knowledge.loading {
        page = page.child(
            text("Loading Knowledge Base…")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    if !state.knowledge.error.is_empty() {
        page = page.child(
            text(state.knowledge.error.clone())
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#e07676")),
        );
    }
    if state.knowledge.records.is_empty() && !state.knowledge.loading {
        page = page.child(
            text("No knowledge records found.")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    for item in &state.knowledge.records {
        let id = item.id.clone();
        let label = format!(
            "{}  {}  {}",
            item.id,
            if item.kind == Kind::Memory {
                "Memory"
            } else if item.status == "lead" {
                "Lead"
            } else {
                "Finding"
            },
            item.title
        );
        page = page.child(
            el().key(format!("knowledge-row-{id}"))
                .w_full()
                .padding(12.0)
                .radius(9.0)
                .fill(theme.color("#202020"))
                .child(
                    col()
                        .gap(4.0)
                        .child(
                            text(label)
                                .font(ui_font(14.0, 500.0))
                                .color(theme.color("#e2e2e2")),
                        )
                        .child(
                            text(format!(
                                "{} · {} · {} evidence",
                                if item.kind == Kind::Memory {
                                    item.role.as_str()
                                } else {
                                    item.status.as_str()
                                },
                                item.summary,
                                item.evidence_count
                            ))
                            .font(ui_font(12.0, 400.0))
                            .color(theme.color("#969696")),
                        ),
                )
                .on_press(move |state: &mut State| select(state, id.clone())),
        );
    }
    if state.knowledge.detail_loading {
        page = page.child(
            text("Loading record…")
                .font(ui_font(14.0, 400.0))
                .color(theme.color("#969696")),
        );
    }
    if let Some(record) = &state.knowledge.selected {
        let status = match &record.data {
            Data::Memory {
                category,
                role,
                support,
            } => format!(
                "Memory · {category}{} · {support}",
                if role.is_empty() {
                    String::new()
                } else {
                    format!(" · {role}")
                }
            ),
            Data::Claim {
                stage,
                disposition,
                rating,
                ..
            } => format!(
                "{} · {stage} · {disposition} · {rating}",
                if stage == "lead" { "Lead" } else { "Finding" }
            ),
        };
        page = page.child(
            col()
                .key("knowledge-detail")
                .w_full()
                .gap(10.0)
                .padding(15.0)
                .radius(10.0)
                .fill(theme.color("#282828"))
                .child(
                    text(format!("{} · {}", record.id, record.title))
                        .font(ui_font(17.0, 600.0))
                        .color(theme.color("#f2f2f2")),
                )
                .child(
                    text(record.summary.clone())
                        .wrap()
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#e2e2e2")),
                )
                .child(
                    text(status)
                        .font(ui_font(12.0, 500.0))
                        .color(theme.color("#bcbcbc")),
                )
                .child(
                    text(record.body.clone())
                        .wrap()
                        .font(ui_font(14.0, 400.0))
                        .color(theme.color("#e2e2e2")),
                )
                .child(
                    text(format!(
                        "Revision {} · {} evidence · {} links",
                        record.revision,
                        record.evidence.len(),
                        record.links.len()
                    ))
                    .font(ui_font(12.0, 400.0))
                    .color(theme.color("#969696")),
                ),
        );
        if let Data::Claim {
            attacker_control,
            violated_invariant,
            observed_behavior,
            impact,
            ..
        } = &record.data
        {
            for (label, value) in [
                ("Attacker influence", attacker_control),
                ("Boundary or invariant", violated_invariant),
                ("Observed behavior", observed_behavior),
                ("Impact", impact),
            ] {
                if !value.is_empty() {
                    page = page.child(
                        text(format!("{label}: {value}"))
                            .wrap()
                            .font(ui_font(12.0, 400.0))
                            .color(theme.color("#bcbcbc")),
                    );
                }
            }
        }
        for evidence in &record.evidence {
            page = page.child(
                text(format!(
                    "{} · {} · tool {}",
                    evidence.id, evidence.summary, evidence.tool_call_id
                ))
                .wrap()
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#bcbcbc")),
            );
        }
        for link in &record.links {
            page = page.child(
                text(format!(
                    "{} → {}: {}",
                    link.relation, link.target_id, link.note
                ))
                .wrap()
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#bcbcbc")),
            );
        }
        for revision in &record.history {
            page = page.child(
                text(format!(
                    "Revision {} · {} · {}",
                    revision.number, revision.actor, revision.reason
                ))
                .wrap()
                .font(ui_font(12.0, 400.0))
                .color(theme.color("#969696")),
            );
        }
        if let Some(offset) = state.knowledge.next_history_offset
            && !state.knowledge.detail_loading
        {
            let id = record.id.clone();
            page = page.child(
                form_button("More History", theme)
                    .key("knowledge-more-history")
                    .on_press(move |state: &mut State| select_page(state, id.clone(), offset)),
            );
        }
    }
    page
}
