//! Optional workspace Runbooks documents and model tools.

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::post,
};
use beale_contracts::runbooks::{
    self, Cell, CellChange, CellEditRequest, CellEditResponse, Document, GetRequest, GetResponse,
    MAX_TITLE_CHARS, QueryRequest, QueryResponse, Summary,
};
use beale_storage::HostDatabase;
use serde::Deserialize;
use serde_json::{Value, json};
mod execution;
pub use execution::{RunOutput, run_document, run_selected_cell};

pub const FEATURE_ID: &str = "beale-runbooks";
pub const FEATURE_NAME: &str = "Runbooks";
pub const FEATURE_DESCRIPTION: &str =
    "Reusable, language-aware workspace procedures with ordered Markdown and code cells.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const TOOL_COUNT: u32 = 6;
pub const PROMPT_EXTENSION: &str = "## Runbooks\n\nUse runbook_query before creating a reusable procedure. A runbook is an ordered document of Markdown and code cells. Keep titles to 64 characters or fewer. Each cell may have a short name; code cells also declare a language, so one runbook can contain multiple languages. Use runbook_get before editing; supply expected_revision and a reason. When editing cells, send the complete ordered cell list: keep existing cell IDs, omit removed cells, and use an empty ID for a new cell. Use runbook_run to execute supported code cells in order, then runbook_status to inspect the recorded result. Code runs with the current host user's privileges in the workspace; use an external VM or container when isolation is needed. Execution output is a candidate artifact, not a verified finding.";

const QUERY: &str = "runbook_query";
const GET: &str = "runbook_get";
const CREATE: &str = "runbook_create";
const EDIT: &str = "runbook_edit";
pub const RUN: &str = "runbook_run";
const STATUS: &str = "runbook_status";

pub fn definitions() -> Vec<Value> {
    let cell = json!({"type":"object","properties":{"kind":{"type":"string","enum":["markdown","code"]},"id":{"type":"string","description":"Empty for a new cell; retain the ID from runbook_get when editing."},"name":{"type":"string","maxLength":40,"description":"Optional short cell name, shown in Canvas and run progress."},"language":{"type":"string","description":"Required for code cells. Language ID such as python, powershell, rust, c++, or swift; each cell may differ."},"source":{"type":"string"}},"required":["kind","id","source"],"additionalProperties":false});
    vec![
        json!({"type":"function","name":QUERY,"description":"Find runbooks in this workspace by title, purpose, or code language.","parameters":{"type":"object","properties":{"text":{"type":"string"},"language":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}),
        json!({"type":"function","name":GET,"description":"Read an entire runbook and its current revision before editing.","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":CREATE,"description":"Create an ordered Markdown/code runbook. A code cell language is descriptive and does not execute code.","parameters":{"type":"object","properties":{"title":{"type":"string","maxLength":MAX_TITLE_CHARS},"purpose":{"type":"string"},"cells":{"type":"array","maxItems":64,"items":cell}},"required":["title","purpose"],"additionalProperties":false}}),
        json!({"type":"function","name":EDIT,"description":"Revise title, purpose, or the complete ordered cells with optimistic revision checking. Supply a reason. Omit cells to leave them unchanged.","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"reason":{"type":"string"},"title":{"type":"string","maxLength":MAX_TITLE_CHARS},"purpose":{"type":"string"},"cells":{"type":"array","maxItems":64,"items":cell}},"required":["id","expected_revision","reason"],"additionalProperties":false}}),
        json!({"type":"function","name":RUN,"description":"Run a saved runbook's code cells in order using their declared interpreters. Markdown cells are skipped. Supports bash/sh, Python, JavaScript/Node, and PowerShell. Runs with the current host user's privileges; a result is not verified evidence.","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":STATUS,"description":"Read a runbook execution's recorded status and bounded output in this session.","parameters":{"type":"object","properties":{"run_id":{"type":"string"}},"required":["run_id"],"additionalProperties":false}}),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(name, QUERY | GET | CREATE | EDIT | RUN | STATUS)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    title: String,
    purpose: String,
    #[serde(default)]
    cells: Vec<Cell>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditInput {
    id: String,
    expected_revision: u64,
    reason: String,
    title: Option<String>,
    purpose: Option<String>,
    cells: Option<Vec<Cell>>,
}

pub fn execute(
    db: &HostDatabase,
    workspace_id: &str,
    session_id: &str,
    name: &str,
    args: &Value,
) -> Result<String> {
    ensure!(
        db.feature_enabled(FEATURE_ID)?
            && db.extension_applies_to("feature", FEATURE_ID, workspace_id)?,
        "Runbooks is disabled for this workspace"
    );
    let result = match name {
        QUERY => {
            let mut request: QueryRequest = serde_json::from_value(
                json!({"workspace_id":workspace_id,"text":args.get("text").cloned().unwrap_or(json!("")),"language":args.get("language").cloned().unwrap_or(json!("")),"limit":args.get("limit").cloned().unwrap_or(json!(20))}),
            )?;
            request.limit = request.limit.min(50);
            json!(QueryResponse {
                runbooks: db.runbook_query(&request)?
            })
        }
        GET => {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .context("id is required")?;
            let runbook = db
                .runbook_get(workspace_id, id)?
                .context("runbook not found")?;
            json!(GetResponse {
                runbook,
                runs: Vec::new(),
                cell_results: Vec::new(),
            })
        }
        CREATE => {
            let input: CreateInput = serde_json::from_value(args.clone())?;
            let document = db.runbook_create(
                Document {
                    id: String::new(),
                    workspace_id: workspace_id.to_owned(),
                    title: input.title,
                    purpose: input.purpose,
                    cells: input.cells,
                    next_cell_number: 1,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    revision: 0,
                },
                "agent",
                session_id,
            )?;
            json!({"runbook":Summary::from(&document)})
        }
        EDIT => {
            let input: EditInput = serde_json::from_value(args.clone())?;
            let mut document = db
                .runbook_get(workspace_id, &input.id)?
                .context("runbook not found")?;
            if let Some(title) = input.title {
                document.title = title;
            }
            if let Some(purpose) = input.purpose {
                document.purpose = purpose;
            }
            if let Some(cells) = input.cells {
                document.cells = cells;
            }
            let document = db.runbook_edit(
                document,
                input.expected_revision,
                "agent",
                session_id,
                &input.reason,
            )?;
            json!({"runbook":Summary::from(&document)})
        }
        STATUS => {
            let id = args
                .get("run_id")
                .and_then(Value::as_str)
                .context("run_id is required")?;
            let run = db
                .runbook_run_status(session_id, id)?
                .context("runbook execution not found")?;
            json!({"run_id":run.id,"runbook_id":run.runbook_id,"revision":run.revision,"status":run.status,"output":run.output})
        }
        _ => anyhow::bail!("unknown Runbooks tool"),
    };
    let output = serde_json::to_string(&result)?;
    ensure!(output.len() <= 128_000, "runbook result is too large");
    Ok(output)
}

#[derive(Clone)]
struct HttpState {
    token: String,
    db: HostDatabase,
}

pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(runbooks::QUERY_PATH, post(query))
        .route(runbooks::GET_PATH, post(get))
        .route(runbooks::CELL_EDIT_PATH, post(edit_cell))
        .with_state(HttpState { token, db })
        .layer(axum::extract::DefaultBodyLimit::max(8_192))
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| {
            value.len() == token.len()
                && value
                    .as_bytes()
                    .iter()
                    .zip(token.as_bytes())
                    .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
                    == 0
        })
}

fn decode_json<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    body: &Bytes,
) -> std::result::Result<T, StatusCode> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/json")
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)
}

fn available(state: &HttpState, workspace_id: &str) -> Result<bool> {
    Ok(state.db.feature_enabled(FEATURE_ID)?
        && state
            .db
            .extension_applies_to("feature", FEATURE_ID, workspace_id)?)
}

async fn query(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<QueryResponse>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut input: QueryRequest = decode_json(&headers, &body)?;
    if !available(&state, &input.workspace_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Err(StatusCode::FORBIDDEN);
    }
    if input.limit == 0 {
        input.limit = 20;
    }
    let runbooks = state
        .db
        .runbook_query(&input)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(QueryResponse { runbooks }))
}

async fn get(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<GetResponse>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let input: GetRequest = decode_json(&headers, &body)?;
    if !available(&state, &input.workspace_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Err(StatusCode::FORBIDDEN);
    }
    let runbook = state
        .db
        .runbook_get(&input.workspace_id, &input.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let runs = match input.session_id.as_deref() {
        Some(session_id) => state
            .db
            .session_runbook_runs(&input.workspace_id, session_id, &input.id)
            .map_err(|_| StatusCode::BAD_REQUEST)?,
        None => Vec::new(),
    };
    let cell_results = match input.session_id.as_deref() {
        Some(session_id) => state
            .db
            .session_runbook_cell_results(&input.workspace_id, session_id, &input.id)
            .map_err(|_| StatusCode::BAD_REQUEST)?,
        None => Vec::new(),
    };
    Ok(Json(GetResponse {
        runbook,
        runs,
        cell_results,
    }))
}

async fn edit_cell(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<CellEditResponse>, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let input: CellEditRequest = decode_json(&headers, &body)?;
    if !available(&state, &input.workspace_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        return Err(StatusCode::FORBIDDEN);
    }
    if input.session_id.is_empty()
        || input.session_id.len() > 128
        || input.cell_id.is_empty()
        || input.cell_id.len() > 128
        || input.expected_revision == 0
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let session = state
        .db
        .session(&input.session_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if session.session.workspace_id != input.workspace_id {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut document = state
        .db
        .runbook_get(&input.workspace_id, &input.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if document.revision != input.expected_revision {
        return Err(StatusCode::CONFLICT);
    }
    let index = document
        .cells
        .iter()
        .position(|cell| cell.id() == input.cell_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let reason = match input.change {
        CellChange::Delete => {
            document.cells.remove(index);
            "Deleted cell"
        }
        CellChange::SetLanguage { language } => {
            let language = language.trim().to_ascii_lowercase();
            if language.is_empty() || language.len() > 40 {
                return Err(StatusCode::BAD_REQUEST);
            }
            let cell = &document.cells[index];
            let source = match cell {
                Cell::Markdown { source, .. } | Cell::Code { source, .. } => source.clone(),
            };
            let name = cell.name().to_owned();
            document.cells[index] = if language == "markdown" {
                Cell::Markdown {
                    id: input.cell_id,
                    name,
                    source,
                }
            } else {
                Cell::Code {
                    id: input.cell_id,
                    name,
                    language,
                    source,
                }
            };
            "Changed cell language"
        }
        CellChange::SetName { name } => {
            let name = name.trim();
            if name.chars().count() > 40 || name.chars().any(char::is_control) {
                return Err(StatusCode::BAD_REQUEST);
            }
            *document.cells[index].name_mut() = name.to_owned();
            "Changed cell name"
        }
    };
    let runbook = state
        .db
        .runbook_edit(
            document,
            input.expected_revision,
            "operator",
            &input.session_id,
            reason,
        )
        .map_err(|_| StatusCode::CONFLICT)?;
    Ok(Json(CellEditResponse { runbook }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use beale_storage::{SessionRow, TurnRow, WorkspaceRow};
    use tower::ServiceExt;

    #[test]
    fn create_and_edit_tools_advertise_the_title_limit() {
        for name in [CREATE, EDIT] {
            let definition = definitions()
                .into_iter()
                .find(|definition| definition["name"] == name)
                .unwrap();
            assert_eq!(
                definition["parameters"]["properties"]["title"]["maxLength"],
                json!(MAX_TITLE_CHARS)
            );
        }
    }

    fn database() -> HostDatabase {
        let root =
            std::env::temp_dir().join(format!("beale-runbooks-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[
            WorkspaceRow {
                id: "workspace-example-a".into(),
                name: "Example A".into(),
                directory: "example-a".into(),
            },
            WorkspaceRow {
                id: "workspace-example-b".into(),
                name: "Example B".into(),
                directory: "example-b".into(),
            },
        ])
        .unwrap();
        db
    }

    #[test]
    fn mixed_languages_revision_and_workspace_boundaries() {
        let db = database();
        assert_eq!(definitions().len(), TOOL_COUNT as usize);
        let create = json!({"title":"Example procedure","purpose":"Review synthetic input handling","cells":[
            {"kind":"markdown","id":"","source":"## Setup"},
            {"kind":"code","id":"","language":"Python","source":"print('example')"},
            {"kind":"code","id":"","language":"PowerShell","source":"Write-Output example"}
        ]});
        assert!(
            execute(
                &db,
                "workspace-example-a",
                "session-example",
                CREATE,
                &create
            )
            .is_err()
        );
        db.set_feature_enabled(FEATURE_ID, true).unwrap();
        let created: Value = serde_json::from_str(
            &execute(
                &db,
                "workspace-example-a",
                "session-example",
                CREATE,
                &create,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(created["runbook"]["id"], "R1");
        assert_eq!(
            created["runbook"]["languages"],
            json!(["python", "powershell"])
        );
        let retrieved: Value = serde_json::from_str(
            &execute(
                &db,
                "workspace-example-a",
                "session-example",
                GET,
                &json!({"id":"R1"}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(retrieved["runbook"]["cells"][0]["id"], "L1");
        assert_eq!(retrieved["runbook"]["cells"][1]["id"], "L2");
        assert!(
            db.runbook_get("workspace-example-b", "R1")
                .unwrap()
                .is_none()
        );
        let edit = json!({"id":"R1","expected_revision":1,"reason":"Keep only the Python check and add Swift","cells":[
            {"kind":"code","id":"L2","language":"python","source":"print('example 2')"},
            {"kind":"code","id":"","language":"swift","source":"print(\"example\")"}
        ]});
        let edited: Value = serde_json::from_str(
            &execute(&db, "workspace-example-a", "session-example", EDIT, &edit).unwrap(),
        )
        .unwrap();
        assert_eq!(edited["runbook"]["revision"], 2);
        assert_eq!(edited["runbook"]["languages"], json!(["python", "swift"]));
        let current = db
            .runbook_get("workspace-example-a", "R1")
            .unwrap()
            .unwrap();
        assert_eq!(current.cells[1].id(), "L4");
        assert!(
            execute(&db, "workspace-example-a", "session-example", EDIT, &edit)
                .unwrap_err()
                .to_string()
                .contains("revision conflict")
        );
        assert!(
            execute(
                &db,
                "workspace-example-a",
                "session-example",
                CREATE,
                &create
            )
            .unwrap_err()
            .to_string()
            .contains("title already exists")
        );
        let queried: Value = serde_json::from_str(
            &execute(
                &db,
                "workspace-example-a",
                "session-example",
                QUERY,
                &json!({"language":"swift"}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(queried["runbooks"].as_array().unwrap().len(), 1);
        let run = db
            .runbook_run_start("workspace-example-a", "session-example", "R1", 2)
            .unwrap();
        let status: Value = serde_json::from_str(
            &execute(
                &db,
                "workspace-example-a",
                "session-example",
                STATUS,
                &json!({"run_id":run.id}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(status["status"], "running");
        db.runbook_run_finish(&run.id, false, "Example failure")
            .unwrap();
        let status: Value = serde_json::from_str(
            &execute(
                &db,
                "workspace-example-a",
                "session-example",
                STATUS,
                &json!({"run_id":run.id}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(status["status"], "failed");
        assert_eq!(status["output"], "Example failure");
        assert!(
            execute(
                &db,
                "workspace-example-a",
                "other-session",
                STATUS,
                &json!({"run_id":run.id})
            )
            .is_err()
        );
        db.set_feature_enabled(FEATURE_ID, false).unwrap();
        assert!(
            execute(
                &db,
                "workspace-example-a",
                "session-example",
                GET,
                &json!({"id":"R1"})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn read_routes_require_auth_and_feature_scope() {
        let db = database();
        db.set_feature_enabled(FEATURE_ID, true).unwrap();
        execute(
            &db,
            "workspace-example-a",
            "session-example",
            CREATE,
            &json!({"title":"Example procedure","purpose":"Synthetic review","cells":[
                {"kind":"code","id":"","name":"Example check","language":"javascript","source":"console.log('example')"}
            ]}),
        )
        .unwrap();
        let run = db
            .runbook_run_start("workspace-example-a", "session-example", "R1", 1)
            .unwrap();
        db.runbook_run_cell_result(&run.id, "L1", true, Some(0), "Example output", "")
            .unwrap();
        db.runbook_run_finish(&run.id, true, "Example output")
            .unwrap();
        let app = router("example-token".into(), db.clone());
        let request = |workspace_id: &str, token: &str| {
            Request::builder()
                .method("POST")
                .uri(runbooks::GET_PATH)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(
                    json!({"workspace_id":workspace_id,"id":"R1","session_id":"session-example"})
                        .to_string(),
                ))
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request("workspace-example-a", "wrong"))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = app
            .clone()
            .oneshot(request("workspace-example-a", "example-token"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 128_000).await.unwrap();
        let data: GetResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(data.runbook.id, "R1");
        assert_eq!(data.runs.len(), 1);
        assert_eq!(data.runs[0].output, "Example output");
        assert_eq!(data.cell_results.len(), 1);
        assert_eq!(data.cell_results[0].cell_id, "L1");
        assert_eq!(data.cell_results[0].status, "completed");
        assert_eq!(data.cell_results[0].stdout, "Example output");
        assert_eq!(data.cell_results[0].exit_code, Some(0));
        db.set_extension_scope(
            "feature",
            FEATURE_ID,
            false,
            &["workspace-example-b".to_owned()],
        )
        .unwrap();
        assert_eq!(
            app.oneshot(request("workspace-example-a", "example-token"))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn cell_edits_require_session_scope_and_current_revision() {
        let db = database();
        db.set_feature_enabled(FEATURE_ID, true).unwrap();
        db.append_turn_with_tools(
            &SessionRow {
                id: "session-example".into(),
                workspace_id: "workspace-example-a".into(),
                title: "Example session".into(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            },
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "example-model".into(),
                prompt: "Synthetic prompt".into(),
                answer: "Synthetic response".into(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
            &[],
        )
        .unwrap();
        execute(
            &db,
            "workspace-example-a",
            "session-example",
            CREATE,
            &json!({"title":"Example procedure","purpose":"Synthetic review","cells":[
                {"kind":"markdown","id":"","source":"Example guidance"},
                {"kind":"code","id":"","name":"Example check","language":"javascript","source":"console.log('example')"}
            ]}),
        )
        .unwrap();
        let app = router("example-token".into(), db.clone());
        let request = |workspace_id: &str, revision: u64, cell_id: &str, change: Value| {
            Request::builder()
                .method("POST")
                .uri(runbooks::CELL_EDIT_PATH)
                .header("content-type", "application/json")
                .header("authorization", "Bearer example-token")
                .body(Body::from(
                    json!({
                        "workspace_id": workspace_id,
                        "session_id": "session-example",
                        "id": "R1",
                        "cell_id": cell_id,
                        "expected_revision": revision,
                        "change": change,
                    })
                    .to_string(),
                ))
                .unwrap()
        };
        let language = json!({"kind":"set_language","language":"Python"});
        assert_eq!(
            app.clone()
                .oneshot(request("workspace-example-b", 1, "L2", language.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND,
        );
        let response = app
            .clone()
            .oneshot(request("workspace-example-a", 1, "L2", language.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 128_000).await.unwrap();
        let edited: CellEditResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(edited.runbook.revision, 2);
        assert!(
            matches!(&edited.runbook.cells[1], Cell::Code { language, .. } if language == "python")
        );
        assert_eq!(edited.runbook.cells[1].name(), "Example check");
        assert_eq!(
            app.clone()
                .oneshot(request("workspace-example-a", 1, "L2", language))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT,
        );
        let response = app
            .clone()
            .oneshot(request(
                "workspace-example-a",
                2,
                "L2",
                json!({"kind":"set_name","name":"Updated check"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 128_000).await.unwrap();
        let renamed: CellEditResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(renamed.runbook.cells[1].name(), "Updated check");
        assert_eq!(renamed.runbook.revision, 3);
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "workspace-example-a",
                    3,
                    "L2",
                    json!({"kind":"set_name","name":"x".repeat(41)}),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
        );
        let response = app
            .oneshot(request(
                "workspace-example-a",
                3,
                "L1",
                json!({"kind":"delete"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 128_000).await.unwrap();
        let deleted: CellEditResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(deleted.runbook.cells.len(), 1);
        assert_eq!(deleted.runbook.cells[0].id(), "L2");
    }
}
