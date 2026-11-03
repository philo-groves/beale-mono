use beale_contracts::{
    PROTOBUF_CONTENT_TYPE, WORKSPACES_PATH,
    v1::{
        CreateWorkspaceRequest, ListWorkspacesResponse, SurfaceCreator, SurfaceKind, SurfaceScope,
        UpdateWorkspaceFoundationRequest, UpdateWorkspaceRulesRequest,
        UpdateWorkspaceSurfacesRequest, WorkspaceCampaign, WorkspaceDetails, WorkspaceFoundation,
        WorkspaceRules, WorkspaceSummary, WorkspaceSurfaceInput, WorkspaceSurfaces,
    },
};
use prost::Message;
use reqwest::{Client, Response, StatusCode, header::CONTENT_TYPE};
use std::{collections::HashSet, net::SocketAddr, path::Path, time::Duration};

use crate::workspace_creation::{
    SurfaceKind as DraftSurfaceKind, SurfaceScope as DraftSurfaceScope, WorkspaceDraft,
};

const MAX_LIST_BYTES: usize = 2_097_152;
const MAX_CREATE_BYTES: usize = 4_096;
const MAX_DETAILS_BYTES: usize = 1_048_576;
pub(crate) const WORKSPACE_CONFLICT_MESSAGE: &str =
    "Workspace changed elsewhere. Reload it before saving.";

#[derive(Clone)]
pub(crate) struct WorkspaceSections {
    pub(crate) details: WorkspaceDetails,
    pub(crate) foundation_revision: u64,
    pub(crate) surfaces_revision: u64,
    pub(crate) rules_revision: u64,
    pub(crate) campaign: WorkspaceCampaign,
}

pub(crate) fn client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize workspace connection".to_owned())
}

pub(crate) fn request_from_draft(
    draft: &WorkspaceDraft,
) -> Result<CreateWorkspaceRequest, &'static str> {
    if let Some(error) = draft.validate_submission() {
        return Err(error);
    }
    Ok(CreateWorkspaceRequest {
        name: draft.name.text.trim().to_owned(),
        directory: draft.directory.text.trim().to_owned(),
        guidance: draft.guidance.text.clone(),
        surfaces: draft
            .surfaces
            .iter()
            .map(|surface| WorkspaceSurfaceInput {
                kind: match surface.kind {
                    DraftSurfaceKind::Repository => SurfaceKind::Repository,
                    DraftSurfaceKind::Documentation => SurfaceKind::Documentation,
                    DraftSurfaceKind::Binary => SurfaceKind::Binary,
                    DraftSurfaceKind::Service => SurfaceKind::Service,
                    DraftSurfaceKind::Domain => SurfaceKind::Domain,
                    DraftSurfaceKind::Other => SurfaceKind::Other,
                } as i32,
                scope: match surface.scope {
                    DraftSurfaceScope::InScope => SurfaceScope::InScope,
                    DraftSurfaceScope::OutOfScope => SurfaceScope::OutOfScope,
                } as i32,
                value: surface.value.trim().to_owned(),
            })
            .collect(),
        rules: draft.rules.clone(),
    })
}

fn url(addr: SocketAddr) -> String {
    format!("http://{addr}{WORKSPACES_PATH}")
}

fn valid_content_type(response: &Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(PROTOBUF_CONTENT_TYPE))
}

async fn bounded_body(mut response: Response, max: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= max.saturating_sub(body.len()) => {
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Ok(Some(_)) => return Err("Workspace response is too large".to_owned()),
            Err(_) => return Err("Could not read workspace response".to_owned()),
        }
    }
}

fn valid_summary(summary: &WorkspaceSummary) -> bool {
    uuid::Uuid::parse_str(&summary.id).is_ok()
        && !summary.name.trim().is_empty()
        && summary.name.chars().count() <= 200
        && !summary.name.chars().any(char::is_control)
        && summary.directory.len() <= 4_096
        && Path::new(&summary.directory).is_absolute()
}

pub(crate) async fn list(
    client: &Client,
    addr: SocketAddr,
) -> Result<Vec<WorkspaceSummary>, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .get(url(addr))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not load workspaces".to_owned())?;
    if response.status() != StatusCode::OK || !valid_content_type(&response) {
        return Err(format!(
            "Workspace list is unavailable ({})",
            response.status()
        ));
    }
    let body = bounded_body(response, MAX_LIST_BYTES).await?;
    decode_list(&body)
}

fn decode_list(body: &[u8]) -> Result<Vec<WorkspaceSummary>, String> {
    let reply =
        ListWorkspacesResponse::decode(body).map_err(|_| "Invalid workspace list".to_owned())?;
    if reply.workspaces.len() > 4_096 {
        return Err("Invalid workspace list".to_owned());
    }
    let mut ids = HashSet::new();
    for workspace in &reply.workspaces {
        if !valid_summary(workspace) || !ids.insert(&workspace.id) {
            return Err("Invalid workspace list".to_owned());
        }
    }
    Ok(reply.workspaces)
}

pub(crate) async fn create(
    client: &Client,
    addr: SocketAddr,
    request: CreateWorkspaceRequest,
) -> Result<WorkspaceSummary, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .post(url(addr))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(request.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not create workspace".to_owned())?;
    match response.status() {
        StatusCode::CREATED if valid_content_type(&response) => {}
        StatusCode::BAD_REQUEST => {
            return Err("The app-server rejected the workspace details.".to_owned());
        }
        StatusCode::CONFLICT => {
            return Err(
                "The workspace directory is already registered or contains files.".to_owned(),
            );
        }
        StatusCode::UNAUTHORIZED => {
            return Err("The app-server rejected local authentication.".to_owned());
        }
        status => return Err(format!("Could not create workspace ({status})")),
    }
    let body = bounded_body(response, MAX_CREATE_BYTES).await?;
    let summary = WorkspaceSummary::decode(body.as_slice())
        .map_err(|_| "Invalid created workspace response".to_owned())?;
    if !valid_summary(&summary) {
        return Err("Invalid created workspace response".to_owned());
    }
    Ok(summary)
}

async fn get_section<M: Message + Default>(
    client: &Client,
    addr: SocketAddr,
    id: uuid::Uuid,
    token: &str,
    suffix: &str,
    max: usize,
) -> Result<M, String> {
    let response = client
        .get(format!("{}/{id}/{suffix}", url(addr)))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not load workspace section".to_owned())?;
    if response.status() != StatusCode::OK || !valid_content_type(&response) {
        return Err(format!(
            "Workspace section is unavailable ({})",
            response.status()
        ));
    }
    let body = bounded_body(response, max).await?;
    M::decode(body.as_slice()).map_err(|_| "Invalid workspace section".to_owned())
}

pub(crate) async fn load_sections(
    client: &Client,
    addr: SocketAddr,
    id: uuid::Uuid,
) -> Result<WorkspaceSections, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let (foundation, surfaces, rules, campaign) = tokio::try_join!(
        get_section::<WorkspaceFoundation>(client, addr, id, &token, "foundation", 25_000),
        get_section::<WorkspaceSurfaces>(client, addr, id, &token, "surfaces", MAX_DETAILS_BYTES),
        get_section::<WorkspaceRules>(client, addr, id, &token, "rules", 262_144),
        get_section::<WorkspaceCampaign>(client, addr, id, &token, "campaign", 4_096),
    )?;
    if foundation.revision == 0 || surfaces.revision == 0 || rules.revision == 0 {
        return Err("Invalid workspace section revisions".to_owned());
    }
    let details = WorkspaceDetails {
        summary: foundation.summary,
        guidance: foundation.guidance,
        surfaces: surfaces.surfaces,
        rules: rules.rules,
    };
    let details = decode_details(id, &details.encode_to_vec())?;
    Ok(WorkspaceSections {
        details,
        foundation_revision: foundation.revision,
        surfaces_revision: surfaces.revision,
        rules_revision: rules.revision,
        campaign,
    })
}

async fn put_section<Q: Message, R: Message + Default>(
    client: &Client,
    addr: SocketAddr,
    id: uuid::Uuid,
    suffix: &str,
    request: Q,
    max: usize,
) -> Result<R, String> {
    let token = crate::provider_auth::authenticated_token(client, addr).await?;
    let response = client
        .put(format!("{}/{id}/{suffix}", url(addr)))
        .bearer_auth(token)
        .header(CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
        .body(request.encode_to_vec())
        .send()
        .await
        .map_err(|_| "Could not save workspace section".to_owned())?;
    match response.status() {
        StatusCode::OK if valid_content_type(&response) => {}
        StatusCode::CONFLICT => {
            return Err(WORKSPACE_CONFLICT_MESSAGE.to_owned());
        }
        StatusCode::BAD_REQUEST => {
            return Err("The app-server rejected these workspace changes.".to_owned());
        }
        status => return Err(format!("Could not save workspace section ({status})")),
    }
    let body = bounded_body(response, max).await?;
    R::decode(body.as_slice()).map_err(|_| "Invalid saved workspace response".to_owned())
}

pub(crate) async fn save_foundation(
    client: &Client,
    addr: SocketAddr,
    id: uuid::Uuid,
    request: UpdateWorkspaceFoundationRequest,
) -> Result<WorkspaceFoundation, String> {
    let saved: WorkspaceFoundation =
        put_section(client, addr, id, "foundation", request, 25_000).await?;
    if saved.revision == 0
        || !saved
            .summary
            .as_ref()
            .is_some_and(|summary| valid_summary(summary) && summary.id == id.to_string())
    {
        return Err("Invalid saved workspace response".to_owned());
    }
    Ok(saved)
}

pub(crate) async fn save_surfaces(
    client: &Client,
    addr: SocketAddr,
    id: uuid::Uuid,
    request: UpdateWorkspaceSurfacesRequest,
) -> Result<WorkspaceSurfaces, String> {
    let saved: WorkspaceSurfaces =
        put_section(client, addr, id, "surfaces", request, MAX_DETAILS_BYTES).await?;
    if saved.revision == 0 || saved.surfaces.len() > 256 {
        return Err("Invalid saved workspace response".to_owned());
    }
    Ok(saved)
}

pub(crate) async fn save_rules(
    client: &Client,
    addr: SocketAddr,
    id: uuid::Uuid,
    request: UpdateWorkspaceRulesRequest,
) -> Result<WorkspaceRules, String> {
    let saved: WorkspaceRules = put_section(client, addr, id, "rules", request, 262_144).await?;
    if saved.revision == 0 || saved.rules.len() > 128 {
        return Err("Invalid saved workspace response".to_owned());
    }
    Ok(saved)
}

fn decode_details(id: uuid::Uuid, body: &[u8]) -> Result<WorkspaceDetails, String> {
    let details =
        WorkspaceDetails::decode(body).map_err(|_| "Invalid workspace details".to_owned())?;
    let valid = details
        .summary
        .as_ref()
        .is_some_and(|summary| valid_summary(summary) && summary.id == id.to_string())
        && details.guidance.len() <= 20_001
        && details.surfaces.len() <= 256
        && details.rules.len() <= 128
        && details.surfaces.iter().all(|surface| {
            matches!(
                SurfaceKind::try_from(surface.kind),
                Ok(SurfaceKind::Repository
                    | SurfaceKind::Documentation
                    | SurfaceKind::Binary
                    | SurfaceKind::Service
                    | SurfaceKind::Domain
                    | SurfaceKind::Other)
            ) && matches!(
                SurfaceScope::try_from(surface.scope),
                Ok(SurfaceScope::InScope | SurfaceScope::OutOfScope)
            ) && matches!(
                SurfaceCreator::try_from(surface.created_by),
                Ok(SurfaceCreator::User | SurfaceCreator::Agent)
            ) && !surface.value.trim().is_empty()
                && surface.value.len() <= 2_048
                && surface.out_of_scope_reason.len() <= 2_000
        });
    if !valid {
        return Err("Invalid workspace details".to_owned());
    }
    Ok(details)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_creation::{Surface, SurfaceCreator as DraftSurfaceCreator};
    use rune::TextEdit;

    #[test]
    fn draft_maps_scope_and_kind_to_contract_without_local_metadata() {
        let mut draft = WorkspaceDraft::default();
        draft.name = TextEdit::new("  Example Workspace  ");
        draft.directory = TextEdit::new(
            std::env::temp_dir()
                .join("example-workspace")
                .to_string_lossy(),
        );
        draft.guidance = TextEdit::new("Use only example.test.");
        draft.surfaces = vec![
            Surface {
                kind: DraftSurfaceKind::Domain,
                scope: DraftSurfaceScope::InScope,
                value: "example.test".to_owned(),
                created_by: DraftSurfaceCreator::User,
                out_of_scope_reason: None,
            },
            Surface {
                kind: DraftSurfaceKind::Documentation,
                scope: DraftSurfaceScope::OutOfScope,
                value: "excluded.example.test".to_owned(),
                created_by: DraftSurfaceCreator::User,
                out_of_scope_reason: Some("Set at workspace creation".to_owned()),
            },
        ];
        draft.rules.push("Keep within scope.".to_owned());
        let request = request_from_draft(&draft).unwrap();
        assert_eq!(request.name, "Example Workspace");
        assert_eq!(request.surfaces[0].kind, SurfaceKind::Domain as i32);
        assert_eq!(request.surfaces[0].scope, SurfaceScope::InScope as i32);
        assert_eq!(request.surfaces[1].kind, SurfaceKind::Documentation as i32);
        assert_eq!(request.surfaces[1].scope, SurfaceScope::OutOfScope as i32);
        assert_eq!(request.rules, ["Keep within scope."]);
        assert_eq!(
            CreateWorkspaceRequest::decode(request.encode_to_vec().as_slice())
                .unwrap()
                .surfaces,
            request.surfaces
        );
    }

    #[test]
    fn rejects_missing_scope_and_invalid_host_summaries() {
        let mut draft = WorkspaceDraft::default();
        draft.name = TextEdit::new("Example Workspace");
        draft.directory = TextEdit::new(std::env::temp_dir().to_string_lossy());
        assert!(request_from_draft(&draft).is_err());
        let invalid = ListWorkspacesResponse {
            workspaces: vec![WorkspaceSummary {
                id: "not-a-uuid".to_owned(),
                name: "Example Workspace".to_owned(),
                directory: std::env::temp_dir().to_string_lossy().into_owned(),
            }],
        };
        assert!(decode_list(&invalid.encode_to_vec()).is_err());
    }

    #[test]
    fn detail_response_must_match_selected_workspace_and_known_surface_types() {
        let id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap();
        let mut details = WorkspaceDetails {
            summary: Some(WorkspaceSummary {
                id: id.to_string(),
                name: "Example Workspace".to_owned(),
                directory: std::env::temp_dir().to_string_lossy().into_owned(),
            }),
            guidance: "Use only example.test.\n".to_owned(),
            surfaces: vec![beale_contracts::v1::WorkspaceSurface {
                kind: SurfaceKind::Domain as i32,
                scope: SurfaceScope::InScope as i32,
                value: "example.test".to_owned(),
                created_by: SurfaceCreator::User as i32,
                out_of_scope_reason: String::new(),
            }],
            rules: vec!["Stay within scope.".to_owned()],
        };
        assert!(valid_summary(details.summary.as_ref().unwrap()));
        let decoded = decode_details(id, &details.encode_to_vec());
        assert!(decoded.is_ok(), "{decoded:?}");
        details.surfaces[0].kind = SurfaceKind::Unspecified as i32;
        assert!(decode_details(id, &details.encode_to_vec()).is_err());
        details.surfaces[0].kind = SurfaceKind::Domain as i32;
        details.summary.as_mut().unwrap().id = uuid::Uuid::new_v4().to_string();
        assert!(decode_details(id, &details.encode_to_vec()).is_err());
    }
}
