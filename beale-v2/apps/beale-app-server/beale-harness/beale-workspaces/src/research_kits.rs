use super::{
    WorkspaceState,
    http::{authorized, checked_body, response},
    validate_surface_update_with_counter,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use beale_contracts::v1::{
    ListResearchKitsResponse, ResearchKitSummary, SetResearchKitEnabledRequest, SurfaceDefinition,
    SurfaceLocator, SurfaceScope, SurfaceScopeState, WorkspaceImportResult, WorkspaceSurfaceInput,
};
use serde_json::{Value, json};
use std::{collections::HashSet, future::Future, time::Duration};

pub(super) const HACKERONE_ID: &str = "hackerone";
const DESCRIPTION: &str =
    "Import a public HackerOne program's policy and complete structured scope for review.";
const SCOPE_QUERY: &str = r#"query BealeScope($handle: String!, $after: String) {
  team(handle: $handle) {
    handle name policy submission_state
    structured_scopes(first: 100, after: $after) {
      total_count
      pageInfo { endCursor hasNextPage }
      nodes {
        asset_type asset_identifier instruction eligible_for_bounty
        eligible_for_submission max_severity url
      }
    }
  }
}"#;

pub(super) fn enabled(state: &WorkspaceState) -> Result<bool, StatusCode> {
    state
        .store
        .db
        .research_kit_enabled(HACKERONE_ID)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn summary(enabled: bool) -> ResearchKitSummary {
    ResearchKitSummary {
        id: HACKERONE_ID.to_owned(),
        name: "HackerOne".to_owned(),
        description: DESCRIPTION.to_owned(),
        enabled,
    }
}

pub(super) async fn list(
    State(state): State<WorkspaceState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(response(ListResearchKitsResponse {
        kits: vec![summary(enabled(&state)?)],
    }))
}

pub(super) async fn set_enabled(
    State(state): State<WorkspaceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: SetResearchKitEnabledRequest = checked_body(&headers, &body)?;
    if request.id != HACKERONE_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    state
        .store
        .db
        .set_research_kit_enabled(HACKERONE_ID, request.enabled)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(response(summary(request.enabled)))
}

pub(super) fn normalize_identifier(value: &str) -> Result<String, String> {
    let value = value.trim();
    let value = value
        .strip_prefix("https://hackerone.com/")
        .or_else(|| value.strip_prefix("https://www.hackerone.com/"))
        .unwrap_or(value)
        .trim_start_matches('@');
    let handle = value.split(['/', '?', '#']).next().unwrap_or("");
    if handle.is_empty()
        || handle.len() > 100
        || !handle
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("Enter a valid HackerOne program identifier.".to_owned());
    }
    Ok(handle.to_owned())
}

pub(super) struct HackerOneImport {
    pub(super) result: WorkspaceImportResult,
    pub(super) facts: Value,
    pub(super) scope_count: usize,
}

pub(super) fn bounded_text(value: &str, max_bytes: usize) -> String {
    let end = value
        .char_indices()
        .map(|(index, character)| index + character.len_utf8())
        .take_while(|end| *end <= max_bytes)
        .last()
        .unwrap_or(0);
    value[..end].to_owned()
}

fn clean_public_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>()
        .trim()
        .to_owned()
}

pub(super) async fn fetch(identifier: &str) -> Result<HackerOneImport, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize HackerOne lookup.".to_owned())?;
    fetch_with(identifier, move |handle, after| {
        let client = client.clone();
        async move {
            let body =
                json!({"query": SCOPE_QUERY, "variables": {"handle": handle, "after": after}});
            let reply = client
                .post("https://hackerone.com/graphql")
                .header("accept", "application/json")
                .header("user-agent", "Beale research kit")
                .json(&body)
                .send()
                .await
                .map_err(|_| "Could not reach HackerOne's public program page.".to_owned())?;
            if !reply.status().is_success() {
                return Err(format!(
                    "HackerOne lookup failed with HTTP {}.",
                    reply.status().as_u16()
                ));
            }
            let bytes = reply
                .bytes()
                .await
                .map_err(|_| "Could not read HackerOne scope.".to_owned())?;
            if bytes.len() > 2_000_000 {
                return Err("HackerOne scope response is too large.".to_owned());
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| "HackerOne returned an invalid scope response.".to_owned())
        }
    })
    .await
}

async fn fetch_with<F, Fut>(identifier: &str, mut page: F) -> Result<HackerOneImport, String>
where
    F: FnMut(String, Option<String>) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let handle = normalize_identifier(identifier)?;
    let mut after: Option<String> = None;
    let mut nodes = Vec::<Value>::new();
    let mut name = String::new();
    let mut policy = String::new();
    let mut submission_state = String::new();
    let mut expected = None;
    for _ in 0..16 {
        let data = page(handle.clone(), after.clone()).await?;
        if data
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| !errors.is_empty())
        {
            return Err("HackerOne could not return this public program scope.".to_owned());
        }
        let team = data
            .pointer("/data/team")
            .filter(|team| team.is_object())
            .ok_or_else(|| "HackerOne program was not found or is not public.".to_owned())?;
        if team.get("handle").and_then(Value::as_str) != Some(handle.as_str()) {
            return Err("HackerOne returned a different program.".to_owned());
        }
        let page_name = team.get("name").and_then(Value::as_str).unwrap_or("");
        if page_name.trim().is_empty()
            || page_name.len() > 200
            || page_name.chars().any(char::is_control)
        {
            return Err("HackerOne returned an invalid program name.".to_owned());
        }
        if name.is_empty() {
            name = page_name.to_owned();
        } else if name != page_name {
            return Err("HackerOne program changed during import. Retry.".to_owned());
        }
        let page_policy =
            clean_public_text(team.get("policy").and_then(Value::as_str).unwrap_or(""));
        if page_policy.len() > 256_000 {
            return Err(
                "HackerOne policy exceeds the import snapshot limit; no draft was imported."
                    .to_owned(),
            );
        }
        let page_submission_state = clean_public_text(
            team.get("submission_state")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        if page_submission_state.len() > 100 {
            return Err("HackerOne returned an invalid submission state.".to_owned());
        }
        if expected.is_some()
            && (policy != page_policy || submission_state != page_submission_state)
        {
            return Err("HackerOne program policy changed during import. Retry.".to_owned());
        }
        policy = page_policy;
        submission_state = page_submission_state;
        let scopes = team
            .get("structured_scopes")
            .ok_or_else(|| "HackerOne returned no structured scope.".to_owned())?;
        let count = usize::try_from(
            scopes
                .get("total_count")
                .and_then(Value::as_u64)
                .ok_or_else(|| "HackerOne did not provide a scope count.".to_owned())?,
        )
        .map_err(|_| "HackerOne scope count is too large.".to_owned())?;
        if count > 256 {
            return Err(
                "This program has more than 256 scope entries; import it manually to avoid loss."
                    .to_owned(),
            );
        }
        if expected.is_some_and(|previous| previous != count) {
            return Err("HackerOne scope changed during import. Retry.".to_owned());
        }
        expected = Some(count);
        let page = scopes
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| "HackerOne returned an incomplete scope page.".to_owned())?;
        nodes.extend(page.iter().cloned());
        let page_info = scopes
            .get("pageInfo")
            .ok_or_else(|| "HackerOne omitted scope pagination.".to_owned())?;
        if page_info.get("hasNextPage").and_then(Value::as_bool) != Some(true) {
            break;
        }
        let cursor = page_info
            .get("endCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .ok_or_else(|| "HackerOne omitted the next scope page.".to_owned())?;
        if after.as_deref() == Some(cursor) {
            return Err("HackerOne repeated a scope page.".to_owned());
        }
        after = Some(cursor.to_owned());
    }
    if nodes.len() != expected.unwrap_or_default() {
        return Err("HackerOne scope was incomplete; no draft was imported.".to_owned());
    }
    let source = format!("https://hackerone.com/{handle}");
    let mut seen = HashSet::new();
    let mut surfaces = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let identifier = node
            .get("asset_identifier")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let asset_type = node
            .get("asset_type")
            .and_then(Value::as_str)
            .unwrap_or("OTHER");
        if identifier.is_empty()
            || identifier.len() > 2_000
            || identifier.chars().any(char::is_control)
            || !seen.insert((
                asset_type.to_ascii_lowercase(),
                identifier.to_ascii_lowercase(),
            ))
        {
            return Err(
                "HackerOne scope contained an empty or duplicate asset; no draft was imported."
                    .to_owned(),
            );
        }
        let eligible = node.get("eligible_for_submission").and_then(Value::as_bool) != Some(false);
        let type_id = if asset_type.contains("SOURCE") {
            "repository"
        } else if asset_type.contains("BINARY") || asset_type.contains("EXECUTABLE") {
            "binary"
        } else if asset_type.contains("DOMAIN")
            || asset_type.contains("URL")
            || identifier.contains('*')
        {
            "domain"
        } else {
            "other"
        };
        let description = clean_public_text(
            node.get("instruction")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        if description.len() > 10_000 {
            return Err(
                "HackerOne scope instruction exceeds the workspace limit; no draft was imported."
                    .to_owned(),
            );
        }
        surfaces.push(WorkspaceSurfaceInput {
            id: String::new(),
            definition: Some(SurfaceDefinition {
                type_id: type_id.to_owned(),
                name: bounded_text(identifier, 200),
                locator: Some(SurfaceLocator {
                    kind: "freeform".to_owned(),
                    value: identifier.to_owned(),
                }),
                scope: if eligible {
                    SurfaceScope::InScope
                } else {
                    SurfaceScope::OutOfScope
                } as i32,
                scope_state: SurfaceScopeState::Candidate as i32,
                description,
                out_of_scope_reason: if eligible {
                    String::new()
                } else {
                    "Listed as ineligible for submission by HackerOne.".to_owned()
                },
                evidence_refs: vec![beale_contracts::v1::SurfaceEvidence {
                    kind: "url".to_owned(),
                    reference: source.clone(),
                    note: "Public HackerOne program scope".to_owned(),
                }],
            }),
            relationships: Vec::new(),
        });
    }
    let rules = vec![
        format!("Verify the current scope and policy at {source} before testing."),
        "Test only assets explicitly authorized by the current program scope.".to_owned(),
        "Follow the program's reporting and disclosure requirements.".to_owned(),
    ];
    let guidance = format!(
        "# HackerOne program: {name}\n\nSource: {source}\nSubmission state: {submission_state}\n\nReview the full imported policy snapshot at references/hackerone-policy.md and verify current terms on HackerOne before testing."
    );
    let facts = json!({"handle":handle,"name":name,"source_url":source,"policy":policy,
        "submission_state":submission_state,"scope_count":nodes.len(),"structured_scopes":nodes});
    validate_surface_update_with_counter(surfaces.clone(), &[], 1).map_err(|_| {
        "HackerOne scope contained invalid or conflicting assets; no draft was imported.".to_owned()
    })?;
    Ok(HackerOneImport {
        scope_count: surfaces.len(),
        result: WorkspaceImportResult {
            name,
            guidance,
            surfaces,
            rules,
            research_kit_id: HACKERONE_ID.to_owned(),
            source_identifier: handle,
            source_policy: policy,
        },
        facts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use prost::Message;
    use tower::ServiceExt;
    use uuid::Uuid;
    #[test]
    fn identifier_accepts_only_a_program_handle_or_hackerone_url() {
        assert_eq!(
            normalize_identifier("example-program").unwrap(),
            "example-program"
        );
        assert_eq!(
            normalize_identifier("https://hackerone.com/example-program/").unwrap(),
            "example-program"
        );
        assert!(normalize_identifier("https://other.example.test/example").is_err());
        assert!(normalize_identifier("../example").is_err());
    }

    #[tokio::test]
    async fn fetches_every_scope_page_and_keeps_exclusions() {
        let result = fetch_with("example", |_handle, after| async move {
            let nodes: Vec<Value> = if after.is_some() {
                vec![json!({"asset_type":"DOMAIN","asset_identifier":"excluded.example.test",
                    "instruction":"Excluded test host.","eligible_for_submission":false})]
            } else {
                (0..100).map(|index| json!({"asset_type":"DOMAIN",
                    "asset_identifier":format!("asset-{index:03}.example.test"),
                    "eligible_for_submission":true})).collect()
            };
            Ok(json!({"data":{"team":{"handle":"example","name":"Example Program",
                "policy":"Stay in scope.","submission_state":"open",
                "structured_scopes":{"total_count":101,"nodes":nodes,
                    "pageInfo":{"hasNextPage":after.is_none(),"endCursor":if after.is_none() { "next" } else { "" }}}}}}))
        }).await.unwrap();
        assert_eq!(result.scope_count, 101);
        assert_eq!(result.result.surfaces.len(), 101);
        assert_eq!(
            result.result.surfaces[100]
                .definition
                .as_ref()
                .unwrap()
                .scope,
            SurfaceScope::OutOfScope as i32
        );
        assert_eq!(result.facts["scope_count"], 101);
    }

    #[tokio::test]
    async fn large_policy_is_preserved_without_overfilling_guidance() {
        let result = fetch_with("example", |_handle, _after| async {
            Ok(
                json!({"data":{"team":{"handle":"example","name":"Example Program",
                "policy":"x".repeat(60_000),"submission_state":"open",
                "structured_scopes":{"total_count":0,"nodes":[],
                    "pageInfo":{"hasNextPage":false,"endCursor":""}}}}}),
            )
        })
        .await;
        let result = result.unwrap();
        assert_eq!(result.result.source_policy.len(), 60_000);
        assert!(result.result.guidance.len() < 20_000);
        let decoded =
            WorkspaceImportResult::decode(result.result.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.source_policy.len(), 60_000);
    }

    #[tokio::test]
    async fn kit_is_enabled_by_default_and_toggle_persists() {
        let root = std::env::temp_dir().join(format!("beale-kit-test-{}", Uuid::new_v4()));
        let token = "ab".repeat(32);
        let store = crate::WorkspaceStore::at(root.clone()).unwrap();
        let app = crate::router(token.clone(), store.clone());
        let get = || {
            Request::builder()
                .uri(beale_contracts::RESEARCH_KITS_PATH)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let reply = app.clone().oneshot(get()).await.unwrap();
        let bytes = axum::body::to_bytes(reply.into_body(), 8_192)
            .await
            .unwrap();
        assert!(ListResearchKitsResponse::decode(bytes).unwrap().kits[0].enabled);
        let request = Request::builder()
            .method("POST")
            .uri(beale_contracts::RESEARCH_KIT_SET_ENABLED_PATH)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(
                SetResearchKitEnabledRequest {
                    id: HACKERONE_ID.to_owned(),
                    enabled: false,
                }
                .encode_to_vec(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
        assert!(!store.db.research_kit_enabled(HACKERONE_ID).unwrap());
        let reply = app.oneshot(get()).await.unwrap();
        let bytes = axum::body::to_bytes(reply.into_body(), 8_192)
            .await
            .unwrap();
        assert!(!ListResearchKitsResponse::decode(bytes).unwrap().kits[0].enabled);
        let _ = std::fs::remove_dir_all(root);
    }
}
