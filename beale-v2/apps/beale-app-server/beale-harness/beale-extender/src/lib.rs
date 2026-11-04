//! Host-owned installation registry for bundled Agent Plugins packages.
//! Installation records a validated package; component activation is separate.

use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use beale_contracts::{
    PLUGIN_INSTALL_PATH, PLUGIN_UNINSTALL_PATH, PLUGINS_PATH, PROTOBUF_CONTENT_TYPE,
    v1::{InstallPluginRequest, ListPluginsResponse, PluginSummary, UninstallPluginRequest},
};
use beale_storage::HostDatabase;
use prost::Message;
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const BUNDLES: [(&str, &str); 4] = [
    ("apple-security-devices", "apple-security-device-plugin"),
    ("apple-target-flags", "apple-target-flags-plugin"),
    ("meta-skills", "meta-skills-plugin"),
    (
        "microsoft-security-devices",
        "microsoft-security-devices-plugin",
    ),
];
const MANIFEST_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "$schema")]
    schema: String,
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Clone)]
struct Catalog {
    root: PathBuf,
}

impl Catalog {
    fn bundled() -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../extensions/plugins")
            .canonicalize()
            .context("locate bundled plugins")?;
        let catalog = Self { root };
        for (name, directory) in BUNDLES {
            catalog.scan(name, directory)?;
        }
        Ok(catalog)
    }

    fn scan(&self, expected_name: &str, directory: &str) -> Result<PluginSummary> {
        let root = self
            .root
            .join(directory)
            .canonicalize()
            .context("locate plugin")?;
        ensure!(
            root.starts_with(&self.root) && root.is_dir(),
            "invalid plugin root"
        );
        let manifest_path = contained_file(&root, &root.join("plugin.json"))?;
        ensure!(
            manifest_path.metadata()?.len() <= 64_000,
            "plugin manifest is too large"
        );
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(manifest_path)?)
            .context("read plugin manifest")?;
        ensure!(
            manifest.schema == MANIFEST_SCHEMA,
            "unsupported plugin schema"
        );
        ensure!(
            valid_name(&manifest.name) && manifest.name == expected_name,
            "invalid plugin name"
        );
        ensure!(
            manifest.description.len() <= 2_000,
            "plugin description is too large"
        );

        let mut skill_count = 0_u32;
        let skills_path = root.join("skills");
        if skills_path.exists() {
            let skills = skills_path.canonicalize()?;
            ensure!(
                skills.starts_with(&root) && skills.is_dir(),
                "invalid skills directory"
            );
            for entry in std::fs::read_dir(skills)? {
                let entry = entry?;
                let skill_dir = entry.path().canonicalize()?;
                ensure!(skill_dir.starts_with(&root), "skill escapes plugin root");
                if skill_dir.is_dir() && skill_dir.join("SKILL.md").exists() {
                    let skill = contained_file(&root, &skill_dir.join("SKILL.md"))?;
                    ensure!(skill.metadata()?.len() <= 1_000_000, "skill is too large");
                    skill_count += 1;
                }
            }
        }

        let mut has_mcp_server = false;
        let mcp_path = root.join("mcp.json");
        if mcp_path.exists() {
            let mcp_path = contained_file(&root, &mcp_path)?;
            ensure!(
                mcp_path.metadata()?.len() <= 64_000,
                "MCP configuration is too large"
            );
            let config: serde_json::Value = serde_json::from_slice(&std::fs::read(mcp_path)?)?;
            ensure!(config["$schema"] == MCP_SCHEMA, "unsupported MCP schema");
            let servers = config["mcpServers"]
                .as_object()
                .context("invalid MCP servers")?;
            has_mcp_server = !servers.is_empty();
        }
        Ok(PluginSummary {
            name: manifest.name,
            description: manifest.description,
            installed: false,
            skill_count,
            has_mcp_server,
        })
    }

    fn list(&self, db: &HostDatabase) -> Result<Vec<PluginSummary>> {
        let installed: HashSet<_> = db.installed_plugin_names()?.into_iter().collect();
        BUNDLES
            .into_iter()
            .map(|(name, directory)| {
                let mut summary = self.scan(name, directory)?;
                summary.installed = installed.contains(name);
                Ok(summary)
            })
            .collect()
    }

    fn install(&self, db: &HostDatabase, name: &str) -> Result<Option<PluginSummary>> {
        let Some((_, directory)) = BUNDLES.iter().find(|(candidate, _)| *candidate == name) else {
            return Ok(None);
        };
        let mut summary = self.scan(name, directory)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let now = i64::try_from(now).context("installation time overflow")?;
        db.install_plugin(name, now)?;
        summary.installed = true;
        Ok(Some(summary))
    }

    fn uninstall(&self, db: &HostDatabase, name: &str) -> Result<Option<PluginSummary>> {
        let Some((_, directory)) = BUNDLES.iter().find(|(candidate, _)| *candidate == name) else {
            return Ok(None);
        };
        let summary = self.scan(name, directory)?;
        db.uninstall_plugin(name)?;
        Ok(Some(summary))
    }
}

fn contained_file(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize().context("locate plugin file")?;
    ensure!(
        path.starts_with(root) && path.is_file(),
        "plugin file escapes root"
    );
    Ok(path)
}

fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && !name.contains("--")
        && !name.contains("..")
}

#[derive(Clone)]
struct HttpState {
    token: String,
    db: HostDatabase,
    catalog: Catalog,
}

pub fn router(token: String, db: HostDatabase) -> Result<Router> {
    let catalog = Catalog::bundled()?;
    Ok(Router::new()
        .route(PLUGINS_PATH, get(list_route))
        .route(PLUGIN_INSTALL_PATH, post(install_route))
        .route(PLUGIN_UNINSTALL_PATH, post(uninstall_route))
        .layer(DefaultBodyLimit::max(1_024))
        .with_state(HttpState { token, db, catalog }))
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

type ProtoResponse = ([(header::HeaderName, &'static str); 1], Vec<u8>);

async fn list_route(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<ProtoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let plugins = state
        .catalog
        .list(&state.db)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        ListPluginsResponse { plugins }.encode_to_vec(),
    ))
}

async fn install_route(
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
    let request = InstallPluginRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let plugin = state
        .catalog
        .install(&state.db, &request.name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        plugin.encode_to_vec(),
    ))
}

async fn uninstall_route(
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
    let request = UninstallPluginRequest::decode(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let plugin = state
        .catalog
        .uninstall(&state.db, &request.name)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok((
        [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
        plugin.encode_to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    #[test]
    fn bundled_catalog_installs_idempotently_and_persists() {
        let catalog = Catalog::bundled().unwrap();
        let root =
            std::env::temp_dir().join(format!("beale-extender-example-{}", std::process::id()));
        let db = HostDatabase::open(&root).unwrap();
        let available = catalog.list(&db).unwrap();
        assert_eq!(available.len(), 4);
        assert!(
            available
                .iter()
                .all(|plugin| !plugin.installed && plugin.skill_count == 1)
        );
        assert!(available[0].has_mcp_server);
        assert!(!available[1].has_mcp_server);
        assert!(catalog.install(&db, "unknown-plugin").unwrap().is_none());
        assert!(
            catalog
                .install(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(
            !catalog
                .uninstall(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(
            !catalog
                .uninstall(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(db.installed_plugin_names().unwrap().is_empty());
        assert!(
            catalog
                .install(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        assert!(
            catalog
                .install(&db, "meta-skills")
                .unwrap()
                .unwrap()
                .installed
        );
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.installed_plugin_names().unwrap(), ["meta-skills"]);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_names() {
        for name in ["", "Upper", "-bad", "bad-", "bad--name", "bad..name"] {
            assert!(!valid_name(name));
        }
        assert!(valid_name("example.plugin-1"));
    }

    #[tokio::test]
    async fn authenticated_protocol_lists_installs_and_uninstalls_bundled_plugin() {
        let root = std::env::temp_dir().join(format!(
            "beale-extender-http-example-{}",
            std::process::id()
        ));
        let db = HostDatabase::open(&root).unwrap();
        let app = router("example-token".to_owned(), db.clone()).unwrap();
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(PLUGINS_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(PLUGINS_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let body = to_bytes(listed.into_body(), 64_000).await.unwrap();
        let catalog = ListPluginsResponse::decode(body).unwrap();
        assert_eq!(catalog.plugins.len(), 4);
        assert!(catalog.plugins.iter().all(|plugin| !plugin.installed));

        let installed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_INSTALL_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        InstallPluginRequest {
                            name: "meta-skills".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(installed.status(), StatusCode::OK);
        let body = to_bytes(installed.into_body(), 64_000).await.unwrap();
        let plugin = PluginSummary::decode(body).unwrap();
        assert_eq!(plugin.name, "meta-skills");
        assert!(plugin.installed);
        assert_eq!(db.installed_plugin_names().unwrap(), ["meta-skills"]);

        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_UNINSTALL_PATH)
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        UninstallPluginRequest {
                            name: "meta-skills".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(db.installed_plugin_names().unwrap(), ["meta-skills"]);

        let uninstalled = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PLUGIN_UNINSTALL_PATH)
                    .header(header::AUTHORIZATION, "Bearer example-token")
                    .header(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)
                    .body(Body::from(
                        UninstallPluginRequest {
                            name: "meta-skills".to_owned(),
                        }
                        .encode_to_vec(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(uninstalled.status(), StatusCode::OK);
        let body = to_bytes(uninstalled.into_body(), 64_000).await.unwrap();
        let plugin = PluginSummary::decode(body).unwrap();
        assert_eq!(plugin.name, "meta-skills");
        assert!(!plugin.installed);
        assert!(db.installed_plugin_names().unwrap().is_empty());
        drop(app);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
