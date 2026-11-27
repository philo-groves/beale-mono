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
        FeatureDetailsResponse, FeatureSummary, GetFeatureDetailsRequest, ListFeaturesResponse,
        SetExtensionWorkspaceScopeRequest, SetFeatureEnabledRequest,
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

/// Catalog display order; features missing from this build are skipped.
const CATALOG_ORDER: [&str; 7] = [
    "webfetch",
    "beale-agent-feedback",
    "swarms",
    "surfaces",
    "beale-findings",
    "beale-memories",
    "beale-runbooks",
];

/// The host catalog, read from each compiled feature's descriptor.
fn feature_specs() -> Vec<FeatureSpec> {
    let mut features = beale_sessions::features::native_features();
    features.sort_by_key(|feature| {
        CATALOG_ORDER
            .iter()
            .position(|id| *id == feature.id)
            .unwrap_or(CATALOG_ORDER.len())
    });
    features
        .into_iter()
        .filter(|feature| !matches!(feature.id, "goal-mode" | "fleet" | "tracking"))
        .map(|feature| FeatureSpec {
            id: feature.id,
            name: feature.name,
            description: feature.description,
            has_prompt_extension: feature.has_prompt_extension(),
            tool_count: feature.tool_count(),
            prompt_extension: feature.prompt_extension,
            tool_names: feature
                .tool_names
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_keep_catalog_order_and_legacy_constants() {
        let ids: Vec<&str> = feature_specs().iter().map(|spec| spec.id).collect();
        let expected: Vec<&str> = CATALOG_ORDER
            .iter()
            .copied()
            .filter(|id| ids.contains(id))
            .collect();
        assert_eq!(ids, expected);
        macro_rules! legacy {
            ($feature:literal, $krate:ident) => {
                #[cfg(feature = $feature)]
                {
                    let descriptor = &$krate::FEATURE;
                    assert_eq!(descriptor.id, $krate::FEATURE_ID);
                    assert_eq!(descriptor.tool_count(), $krate::TOOL_COUNT);
                    assert_eq!(
                        descriptor.has_prompt_extension(),
                        $krate::HAS_PROMPT_EXTENSION
                    );
                }
            };
        }
        legacy!("webfetch", beale_webfetch);
        legacy!("agent-feedback", beale_agent_feedback);
        legacy!("swarms", beale_swarms);
        for descriptor in [
            &beale_goals::FEATURE,
            &beale_fleet::FEATURE,
            &beale_tracking::FEATURE,
        ] {
            assert_eq!(descriptor.tool_count(), descriptor.tool_names.len() as u32);
        }
        assert_eq!(beale_goals::FEATURE.tool_names, &["goal_status"]);
        assert!(beale_fleet::FEATURE.tool_names.is_empty());
        assert!(beale_tracking::FEATURE.tool_names.is_empty());
        legacy!("surfaces", beale_surfaces);
        legacy!("findings", beale_findings);
        legacy!("memories", beale_memories);
        legacy!("runbooks", beale_runbooks);
    }

    use axum::{body::Body, http::Request};
    #[cfg(feature = "webfetch")]
    use beale_storage::WorkspaceRow;
    use tower::ServiceExt;

    #[cfg(not(feature = "webfetch"))]
    #[test]
    fn webfetch_is_absent_when_not_compiled() {
        assert!(feature_spec("webfetch").is_none());
    }

    #[test]
    fn feature_details_tool_names_match_summary_counts() {
        let specs = feature_specs();
        assert!(specs.iter().all(|spec| spec.id != "beale-knowledge"));
        for spec in specs {
            assert_eq!(
                spec.tool_names.len(),
                spec.tool_count as usize,
                "{}",
                spec.id
            );
        }
    }

    #[cfg(feature = "webfetch")]
    #[tokio::test]
    async fn feature_is_off_by_default_and_toggle_is_authenticated() {
        for id in [
            #[cfg(feature = "webfetch")]
            beale_webfetch::FEATURE_ID,
        ] {
            let root = std::env::temp_dir()
                .join(format!("beale-feature-example-{}", uuid::Uuid::new_v4()));
            let db = HostDatabase::open(&root).unwrap();
            beale_sessions::features::migrate(&db).unwrap();
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
            #[cfg(feature = "swarms")]
            {
                let swarms = catalog
                    .features
                    .iter()
                    .find(|feature| feature.id == beale_swarms::FEATURE_ID)
                    .unwrap();
                assert!(!swarms.enabled);
                assert_eq!(swarms.tool_count, beale_swarms::TOOL_COUNT);
                assert!(!swarms.has_prompt_extension);
            }
            let feature = catalog
                .features
                .iter()
                .find(|feature| feature.id == id)
                .unwrap();
            assert!(!feature.enabled);
            assert!(feature.has_prompt_extension);
            assert_eq!(feature.tool_count, 1);
            let denied = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(FEATURE_DETAILS_PATH)
                        .header("content-type", PROTOBUF_CONTENT_TYPE)
                        .body(Body::from(
                            GetFeatureDetailsRequest { id: id.to_owned() }.encode_to_vec(),
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
                            GetFeatureDetailsRequest { id: id.to_owned() }.encode_to_vec(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            let bytes = axum::body::to_bytes(response.into_body(), 4_096)
                .await
                .unwrap();
            let details = FeatureDetailsResponse::decode(bytes).unwrap();
            assert_eq!(details.id, id);
            let spec = feature_spec(id).unwrap();
            assert_eq!(details.tool_names, spec.tool_names);
            assert_eq!(details.prompt_extension, spec.prompt_extension);
            let denied = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(FEATURE_SET_ENABLED_PATH)
                        .header("content-type", PROTOBUF_CONTENT_TYPE)
                        .body(Body::from(
                            SetFeatureEnabledRequest {
                                id: id.to_owned(),
                                enabled: true,
                            }
                            .encode_to_vec(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
            assert!(!db.feature_enabled(id).unwrap());
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
                                id: id.to_owned(),
                                enabled: true,
                            }
                            .encode_to_vec(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(db.feature_enabled(id).unwrap());
            let workspace_id = uuid::Uuid::new_v4().to_string();
            db.replace_workspaces(&[WorkspaceRow {
                id: workspace_id.clone(),
                name: "Example workspace".to_owned(),
                directory: "example-directory".to_owned(),
            }])
            .unwrap();
            let scope_request = SetExtensionWorkspaceScopeRequest {
                id: id.to_owned(),
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
                db.extension_scope("feature", id).unwrap().workspace_ids,
                [workspace_id]
            );
            drop(db);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn standard_harness_components_are_not_catalog_features() {
        let root =
            std::env::temp_dir().join(format!("beale-standard-example-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        beale_sessions::features::migrate(&db).unwrap();
        let app = router("example-token".to_owned(), db.clone());
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
        let catalog = ListFeaturesResponse::decode(
            axum::body::to_bytes(response.into_body(), 4_096)
                .await
                .unwrap(),
        )
        .unwrap();
        for id in ["goal-mode", "fleet", "tracking"] {
            assert!(!catalog.features.iter().any(|feature| feature.id == id));
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
                                id: id.to_owned(),
                                enabled: false,
                            }
                            .encode_to_vec(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            for (path, body) in [
                (
                    FEATURE_DETAILS_PATH,
                    GetFeatureDetailsRequest { id: id.to_owned() }.encode_to_vec(),
                ),
                (
                    FEATURE_SCOPE_PATH,
                    SetExtensionWorkspaceScopeRequest {
                        id: id.to_owned(),
                        workspace_ids: Vec::new(),
                        all_workspaces: true,
                    }
                    .encode_to_vec(),
                ),
            ] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(path)
                            .header("authorization", "Bearer example-token")
                            .header("content-type", PROTOBUF_CONTENT_TYPE)
                            .body(Body::from(body))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::NOT_FOUND);
            }
        }
        drop(app);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
