use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    FEATURE_DETAILS_PATH, FEATURE_SET_ENABLED_PATH, FEATURES_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{
        FeatureDetailsResponse, FeatureSummary, GetFeatureDetailsRequest, GoalStatus,
        ListFeaturesResponse, SetFeatureEnabledRequest,
    },
};
use beale_storage::HostDatabase;
use prost::Message;

use crate::FEATURE_ID;

const FEATURE_NAME: &str = "Goal Mode";
const FEATURE_DESCRIPTION: &str = "Keep working toward a New Research objective across turns, with optional turn and duration limits.";

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
        .layer(DefaultBodyLimit::max(1_024))
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

fn summary(enabled: bool) -> FeatureSummary {
    FeatureSummary {
        id: FEATURE_ID.to_owned(),
        name: FEATURE_NAME.to_owned(),
        description: FEATURE_DESCRIPTION.to_owned(),
        enabled,
        has_prompt_extension: crate::HAS_PROMPT_EXTENSION,
        tool_count: crate::TOOL_COUNT,
    }
}

async fn list(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let enabled = state
        .db
        .feature_enabled(FEATURE_ID)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        ListFeaturesResponse {
            features: vec![summary(enabled)],
        }
        .encode_to_vec(),
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
    if request.id != FEATURE_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        FeatureDetailsResponse {
            id: FEATURE_ID.to_owned(),
            name: FEATURE_NAME.to_owned(),
            description: FEATURE_DESCRIPTION.to_owned(),
            prompt_extension: crate::PROMPT_EXTENSION.to_owned(),
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
    if request.id != FEATURE_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    state
        .db
        .set_feature_enabled(FEATURE_ID, request.enabled)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !request.enabled {
        state
            .db
            .pause_active_goals(GoalStatus::Active as i32, GoalStatus::Paused as i32)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        summary(request.enabled).encode_to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use beale_storage::{GoalRow, SessionRow, TurnActivity, TurnRow};
    use tower::ServiceExt;

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
        let bytes = axum::body::to_bytes(response.into_body(), 1_024)
            .await
            .unwrap();
        let catalog = ListFeaturesResponse::decode(bytes).unwrap();
        assert_eq!(catalog.features.len(), 1);
        assert!(!catalog.features[0].enabled);
        assert!(catalog.features[0].has_prompt_extension);
        assert_eq!(catalog.features[0].tool_count, 1);
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
        assert_eq!(details.prompt_extension, crate::PROMPT_EXTENSION);
        assert!(
            crate::system_section(crate::Limits::default(), 0, 0)
                .starts_with(&details.prompt_extension)
        );
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
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

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
}
