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
    self, Cell, Document, GetRequest, GetResponse, QueryRequest, QueryResponse, Summary,
};
use beale_storage::HostDatabase;
use serde::Deserialize;
use serde_json::{Value, json};

pub const FEATURE_ID: &str = "beale-runbooks";
pub const FEATURE_NAME: &str = "Runbooks";
pub const FEATURE_DESCRIPTION: &str =
    "Reusable, language-aware workspace procedures with ordered Markdown and code cells.";
pub const HAS_PROMPT_EXTENSION: bool = true;
pub const TOOL_COUNT: u32 = 4;
pub const PROMPT_EXTENSION: &str = "## Runbooks\n\nUse runbook_query before creating a reusable procedure. A runbook is an ordered document of Markdown and code cells. Each code cell declares its own language; a runbook can contain multiple languages. The language names describe source code and do not grant execution. Use runbook_get before editing; supply expected_revision and a reason. When editing cells, send the complete ordered cell list: keep existing cell IDs, omit removed cells, and use an empty ID for a new cell. Runbooks do not execute yet and are not evidence of a target observation.";

const QUERY: &str = "runbook_query";
const GET: &str = "runbook_get";
const CREATE: &str = "runbook_create";
const EDIT: &str = "runbook_edit";

pub fn definitions() -> Vec<Value> {
    let cell = json!({"type":"object","properties":{"kind":{"type":"string","enum":["markdown","code"]},"id":{"type":"string","description":"Empty for a new cell; retain the ID from runbook_get when editing."},"language":{"type":"string","description":"Required for code cells. Language ID such as python, powershell, rust, c++, or swift; each cell may differ."},"source":{"type":"string"}},"required":["kind","id","source"],"additionalProperties":false});
    vec![
        json!({"type":"function","name":QUERY,"description":"Find runbooks in this workspace by title, purpose, or code language.","parameters":{"type":"object","properties":{"text":{"type":"string"},"language":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"additionalProperties":false}}),
        json!({"type":"function","name":GET,"description":"Read an entire runbook and its current revision before editing.","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}}),
        json!({"type":"function","name":CREATE,"description":"Create an ordered Markdown/code runbook. A code cell language is descriptive and does not execute code.","parameters":{"type":"object","properties":{"title":{"type":"string"},"purpose":{"type":"string"},"cells":{"type":"array","maxItems":64,"items":cell}},"required":["title","purpose"],"additionalProperties":false}}),
        json!({"type":"function","name":EDIT,"description":"Revise title, purpose, or the complete ordered cells with optimistic revision checking. Supply a reason. Omit cells to leave them unchanged.","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"integer","minimum":1},"reason":{"type":"string"},"title":{"type":"string"},"purpose":{"type":"string"},"cells":{"type":"array","maxItems":64,"items":cell}},"required":["id","expected_revision","reason"],"additionalProperties":false}}),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(name, QUERY | GET | CREATE | EDIT)
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
            json!(GetResponse { runbook })
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
    Ok(Json(GetResponse { runbook }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use beale_storage::WorkspaceRow;
    use tower::ServiceExt;

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
            &json!({"title":"Example procedure","purpose":"Synthetic review"}),
        )
        .unwrap();
        let app = router("example-token".into(), db.clone());
        let request = |workspace_id: &str, token: &str| {
            Request::builder()
                .method("POST")
                .uri(runbooks::GET_PATH)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(
                    json!({"workspace_id":workspace_id,"id":"R1"}).to_string(),
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
}
