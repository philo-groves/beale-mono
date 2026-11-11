use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    FEATURE_DETAILS_PATH, FEATURE_SCOPE_PATH, FEATURE_SET_ENABLED_PATH, FEATURES_PATH,
    PROTOBUF_CONTENT_TYPE,
    v1::{
        FeatureDetailsResponse, FeatureSummary, GetFeatureDetailsRequest, GoalStatus,
        ListFeaturesResponse, SetExtensionWorkspaceScopeRequest, SetFeatureEnabledRequest,
    },
};
use beale_storage::HostDatabase;
use prost::Message;

struct FeatureSpec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    has_prompt_extension: bool,
    tool_count: u32,
    prompt_extension: &'static str,
    tool_names: Vec<String>,
}

#[cfg(any(
    feature = "surfaces",
    feature = "knowledge",
    feature = "findings",
    feature = "memories",
    feature = "runbooks"
))]
fn definition_names(definitions: Vec<serde_json::Value>) -> Vec<String> {
    definitions
        .iter()
        .filter_map(|definition| definition.get("name")?.as_str().map(str::to_owned))
        .collect()
}

fn feature_specs() -> Vec<FeatureSpec> {
    vec![
        #[cfg(feature = "goal-mode")]
        FeatureSpec {
            id: beale_goals::FEATURE_ID,
            name: beale_goals::FEATURE_NAME,
            description: beale_goals::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_goals::HAS_PROMPT_EXTENSION,
            tool_count: beale_goals::TOOL_COUNT,
            prompt_extension: beale_goals::PROMPT_EXTENSION,
            tool_names: vec![beale_goals::TOOL_NAME.to_owned()],
        },
        #[cfg(feature = "fleet")]
        FeatureSpec {
            id: beale_fleet::FEATURE_ID,
            name: beale_fleet::FEATURE_NAME,
            description: beale_fleet::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_fleet::HAS_PROMPT_EXTENSION,
            tool_count: beale_fleet::TOOL_COUNT,
            prompt_extension: beale_fleet::PROMPT_EXTENSION,
            tool_names: Vec::new(),
        },
        #[cfg(feature = "graphs")]
        FeatureSpec {
            id: beale_graphs::FEATURE_ID,
            name: beale_graphs::FEATURE_NAME,
            description: beale_graphs::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_graphs::HAS_PROMPT_EXTENSION,
            tool_count: beale_graphs::TOOL_COUNT,
            prompt_extension: beale_graphs::PROMPT_EXTENSION,
            tool_names: vec![beale_graphs::DELEGATE_TOOL.to_owned()],
        },
        #[cfg(feature = "tracking")]
        FeatureSpec {
            id: beale_tracking::FEATURE_ID,
            name: beale_tracking::FEATURE_NAME,
            description: beale_tracking::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_tracking::HAS_PROMPT_EXTENSION,
            tool_count: beale_tracking::TOOL_COUNT,
            prompt_extension: beale_tracking::PROMPT_EXTENSION,
            tool_names: Vec::new(),
        },
        #[cfg(feature = "surfaces")]
        FeatureSpec {
            id: beale_surfaces::FEATURE_ID,
            name: beale_surfaces::FEATURE_NAME,
            description: beale_surfaces::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_surfaces::HAS_PROMPT_EXTENSION,
            tool_count: beale_surfaces::TOOL_COUNT,
            prompt_extension: beale_surfaces::PROMPT_EXTENSION,
            tool_names: definition_names(beale_surfaces::definitions()),
        },
        #[cfg(feature = "knowledge")]
        FeatureSpec {
            id: beale_knowledge::FEATURE_ID,
            name: beale_knowledge::FEATURE_NAME,
            description: beale_knowledge::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_knowledge::HAS_PROMPT_EXTENSION,
            tool_count: beale_knowledge::TOOL_COUNT,
            prompt_extension: beale_knowledge::PROMPT_EXTENSION,
            tool_names: definition_names(beale_knowledge::definitions()),
        },
        #[cfg(feature = "findings")]
        FeatureSpec {
            id: beale_findings::FEATURE_ID,
            name: beale_findings::FEATURE_NAME,
            description: beale_findings::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_findings::HAS_PROMPT_EXTENSION,
            tool_count: beale_findings::TOOL_COUNT,
            prompt_extension: beale_findings::PROMPT_EXTENSION,
            tool_names: definition_names(beale_findings::definitions()),
        },
        #[cfg(feature = "memories")]
        FeatureSpec {
            id: beale_memories::FEATURE_ID,
            name: beale_memories::FEATURE_NAME,
            description: beale_memories::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_memories::HAS_PROMPT_EXTENSION,
            tool_count: beale_memories::TOOL_COUNT,
            prompt_extension: beale_memories::PROMPT_EXTENSION,
            tool_names: definition_names(beale_memories::definitions()),
        },
        #[cfg(feature = "runbooks")]
        FeatureSpec {
            id: beale_runbooks::FEATURE_ID,
            name: beale_runbooks::FEATURE_NAME,
            description: beale_runbooks::FEATURE_DESCRIPTION,
            has_prompt_extension: beale_runbooks::HAS_PROMPT_EXTENSION,
            tool_count: beale_runbooks::TOOL_COUNT,
            prompt_extension: beale_runbooks::PROMPT_EXTENSION,
            tool_names: definition_names(beale_runbooks::definitions()),
        },
    ]
}

fn feature_spec(id: &str) -> Option<FeatureSpec> {
    feature_specs().into_iter().find(|spec| spec.id == id)
}

#[derive(Clone)]
struct HttpState {
    token: String,
    db: HostDatabase,
}

type ProtoResponse = ([(header::HeaderName, &'static str); 1], Vec<u8>);

pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(FEATURES_PATH, get(list))
        .route(FEATURE_DETAILS_PATH, post(details))
        .route(FEATURE_SET_ENABLED_PATH, post(set_enabled))
        .route(FEATURE_SCOPE_PATH, post(set_scope))
        .layer(DefaultBodyLimit::max(8_192))
        .with_state(HttpState { token, db })
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(presented) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    presented.len() == token.len()
        && presented
            .as_bytes()
            .iter()
            .zip(token.as_bytes())
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn summary(
    spec: &FeatureSpec,
    enabled: bool,
    scope: beale_storage::ExtensionWorkspaceScope,
) -> FeatureSummary {
    FeatureSummary {
        id: spec.id.to_owned(),
        name: spec.name.to_owned(),
        description: spec.description.to_owned(),
        enabled,
        has_prompt_extension: spec.has_prompt_extension,
        tool_count: spec.tool_count,
        workspace_ids: scope.workspace_ids,
        all_workspaces: scope.all_workspaces,
    }
}

async fn list(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let features = feature_specs()
        .iter()
        .map(|spec| {
            let enabled = state
                .db
                .feature_enabled(spec.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let scope = state
                .db
                .extension_scope("feature", spec.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(summary(spec, enabled, scope))
        })
        .collect::<Result<Vec<_>, StatusCode>>()?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        ListFeaturesResponse { features }.encode_to_vec(),
    ))
}

async fn details(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request = GetFeatureDetailsRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let spec = feature_spec(&request.id).ok_or(StatusCode::NOT_FOUND)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        FeatureDetailsResponse {
            id: spec.id.to_owned(),
            name: spec.name.to_owned(),
            description: spec.description.to_owned(),
            prompt_extension: spec.prompt_extension.to_owned(),
            tool_names: spec.tool_names,
        }
        .encode_to_vec(),
    ))
}

async fn set_enabled(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request = SetFeatureEnabledRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let spec = feature_spec(&request.id).ok_or(StatusCode::NOT_FOUND)?;
    state
        .db
        .set_feature_enabled(spec.id, request.enabled)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !request.enabled && spec.id == "goal-mode" {
        state
            .db
            .pause_active_goals(GoalStatus::Active as i32, GoalStatus::Paused as i32)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let scope = state
        .db
        .extension_scope("feature", spec.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        summary(&spec, request.enabled, scope).encode_to_vec(),
    ))
}

async fn set_scope(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(PROTOBUF_CONTENT_TYPE)
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request =
        SetExtensionWorkspaceScopeRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let spec = feature_spec(&request.id).ok_or(StatusCode::NOT_FOUND)?;
    if spec.id == "tracking" && (!request.all_workspaces || !request.workspace_ids.is_empty()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !state
        .db
        .feature_enabled(spec.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::CONFLICT);
    }
    state
        .db
        .set_extension_scope(
            "feature",
            spec.id,
            request.all_workspaces,
            &request.workspace_ids,
        )
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if !request.all_workspaces && spec.id == "goal-mode" {
        state
            .db
            .pause_active_goals_outside_feature_scope(
                spec.id,
                GoalStatus::Active as i32,
                GoalStatus::Paused as i32,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let scope = state
        .db
        .extension_scope("feature", spec.id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        SetExtensionWorkspaceScopeRequest {
            id: spec.id.to_owned(),
            workspace_ids: scope.workspace_ids,
            all_workspaces: scope.all_workspaces,
        }
        .encode_to_vec(),
    ))
}

#[cfg(all(test, any(feature = "goal-mode", feature = "fleet")))]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    #[cfg(feature = "goal-mode")]
    use beale_goals::FEATURE_ID;
    #[cfg(feature = "goal-mode")]
    use beale_storage::{GoalRow, SessionRow, TurnActivity, TurnRow, WorkspaceRow};
    use tower::ServiceExt;

    #[test]
    fn feature_details_tool_names_match_summary_counts() {
        for spec in feature_specs() {
            assert_eq!(
                spec.tool_names.len(),
                spec.tool_count as usize,
                "{}",
                spec.id
            );
        }
    }

    #[cfg(feature = "goal-mode")]
    #[tokio::test]
    async fn feature_is_off_by_default_and_toggle_is_authenticated() {
        let root =
            std::env::temp_dir().join(format!("beale-feature-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let app = router("example-token".to_owned(), db.clone());
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(FEATURES_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(FEATURES_PATH)
                    .header("authorization", "Bearer example-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4_096)
            .await
            .unwrap();
        let catalog = ListFeaturesResponse::decode(bytes).unwrap();
        #[cfg(feature = "surfaces")]
        {
            let surfaces = catalog
                .features
                .iter()
                .find(|feature| feature.id == beale_surfaces::FEATURE_ID)
                .unwrap();
            assert!(!surfaces.enabled);
            assert_eq!(surfaces.tool_count, beale_surfaces::TOOL_COUNT);
        }
        #[cfg(feature = "graphs")]
        {
            let graphs = catalog
                .features
                .iter()
                .find(|feature| feature.id == beale_graphs::FEATURE_ID)
                .unwrap();
            assert!(!graphs.enabled);
            assert_eq!(graphs.tool_count, 1);
            assert!(graphs.has_prompt_extension);
        }
        let goal = catalog
            .features
            .iter()
            .find(|feature| feature.id == FEATURE_ID)
            .unwrap();
        assert!(!goal.enabled);
        assert!(goal.has_prompt_extension);
        assert_eq!(goal.tool_count, 1);
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_DETAILS_PATH)
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        GetFeatureDetailsRequest {
                            id: FEATURE_ID.to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_DETAILS_PATH)
                    .header("authorization", "Bearer example-token")
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        GetFeatureDetailsRequest {
                            id: FEATURE_ID.to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4_096)
            .await
            .unwrap();
        let details = FeatureDetailsResponse::decode(bytes).unwrap();
        assert_eq!(details.id, FEATURE_ID);
        assert_eq!(details.tool_names, [beale_goals::TOOL_NAME]);
        assert_eq!(details.prompt_extension, beale_goals::PROMPT_EXTENSION);
        assert!(
            beale_goals::system_section(beale_goals::Limits::default(), 0, 0)
                .starts_with(&details.prompt_extension)
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_SET_ENABLED_PATH)
                    .header("authorization", "Bearer example-token")
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        SetFeatureEnabledRequest {
                            id: FEATURE_ID.to_owned(),
                            enabled: true,
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(db.feature_enabled(FEATURE_ID).unwrap());
        let workspace_id = uuid::Uuid::new_v4().to_string();
        db.replace_workspaces(&[WorkspaceRow {
            id: workspace_id.clone(),
            name: "Example workspace".to_owned(),
            directory: "example-directory".to_owned(),
        }])
        .unwrap();
        let scope_request = SetExtensionWorkspaceScopeRequest {
            id: FEATURE_ID.to_owned(),
            workspace_ids: vec![workspace_id.clone()],
            all_workspaces: false,
        };
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_SCOPE_PATH)
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(scope_request.encode_to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_SCOPE_PATH)
                    .header("authorization", "Bearer example-token")
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(scope_request.encode_to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            db.extension_scope("feature", FEATURE_ID)
                .unwrap()
                .workspace_ids,
            [workspace_id]
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "goal-mode")]
    #[tokio::test]
    async fn disabling_goal_mode_pauses_saved_active_goals() {
        let root =
            std::env::temp_dir().join(format!("beale-feature-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        db.set_feature_enabled(FEATURE_ID, true).unwrap();
        let session = SessionRow {
            id: uuid::Uuid::new_v4().to_string(),
            workspace_id: uuid::Uuid::new_v4().to_string(),
            title: "Example objective".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example objective".to_owned(),
            answer: "Example progress".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        let goal = GoalRow {
            session_id: session.id.clone(),
            objective: turn.prompt.clone(),
            status: GoalStatus::Active as i32,
            turns_used: 1,
            required_state: String::new(),
            max_turns: 0,
            max_duration_minutes: 0,
            active_ms: 1,
        };
        db.append_turn_with_goal_activity(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: Some(&goal),
            },
        )
        .unwrap();
        let app = router("example-token".to_owned(), db.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_SET_ENABLED_PATH)
                    .header("authorization", "Bearer example-token")
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        SetFeatureEnabledRequest {
                            id: FEATURE_ID.to_owned(),
                            enabled: false,
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!db.feature_enabled(FEATURE_ID).unwrap());
        assert_eq!(
            db.goal(&session.id).unwrap().unwrap().status,
            GoalStatus::Paused as i32
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "fleet")]
    #[tokio::test]
    async fn fleet_starts_disabled_and_enablement_persists() {
        let root =
            std::env::temp_dir().join(format!("beale-fleet-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let app = router("example-token".to_owned(), db.clone());
        let catalog = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(FEATURES_PATH)
                    .header("authorization", "Bearer example-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(catalog.into_body(), 4_096)
            .await
            .unwrap();
        let catalog = ListFeaturesResponse::decode(bytes).unwrap();
        let fleet = catalog
            .features
            .iter()
            .find(|feature| feature.id == beale_fleet::FEATURE_ID)
            .unwrap();
        assert!(!fleet.enabled);
        assert!(!fleet.has_prompt_extension);
        assert_eq!(fleet.tool_count, 0);
        let details = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_DETAILS_PATH)
                    .header("authorization", "Bearer example-token")
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        GetFeatureDetailsRequest {
                            id: beale_fleet::FEATURE_ID.to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let details = FeatureDetailsResponse::decode(
            axum::body::to_bytes(details.into_body(), 4_096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(details.id, beale_fleet::FEATURE_ID);
        assert!(details.prompt_extension.is_empty());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(FEATURE_SET_ENABLED_PATH)
                    .header("authorization", "Bearer example-token")
                    .header("content-type", PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        SetFeatureEnabledRequest {
                            id: beale_fleet::FEATURE_ID.to_owned(),
                            enabled: true,
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert!(reopened.feature_enabled(beale_fleet::FEATURE_ID).unwrap());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
