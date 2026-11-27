use super::*;

struct Fixture {
    root: PathBuf,
    runtime: PluginRuntime,
    package: PathBuf,
}
impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "beale-plugin-resources-{label}-{}",
            std::process::id()
        ));
        let db = HostDatabase::open(&root).unwrap();
        let runtime = PluginRuntime::new(db.clone(), root.clone()).unwrap();
        let package = root.join("plugins/packages/example-plugin");
        std::fs::create_dir_all(package.join("skills/example/scripts")).unwrap();
        std::fs::create_dir_all(package.join("skills/example/references")).unwrap();
        std::fs::create_dir_all(package.join("skills/example/assets/nested")).unwrap();
        std::fs::write(package.join("plugin.json"),serde_json::json!({"$schema":MANIFEST_SCHEMA,"name":"example-plugin","description":"Synthetic resource fixture"}).to_string()).unwrap();
        std::fs::write(
            package.join("mcp.json"),
            serde_json::json!({"$schema":MCP_SCHEMA,"mcpServers":{}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            package.join("skills/example/SKILL.md"),
            b"Read references/example.md and run scripts/example.py.",
        )
        .unwrap();
        std::fs::write(
            package.join("skills/example/scripts/example.py"),
            b"print('example')\n",
        )
        .unwrap();
        std::fs::write(
            package.join("skills/example/references/example.md"),
            b"Synthetic example.test guidance",
        )
        .unwrap();
        std::fs::write(
            package.join("skills/example/assets/nested/example.bin"),
            [0, 128, 255],
        )
        .unwrap();
        std::fs::write(
            package.join("private-settings.json"),
            b"synthetic-private-value",
        )
        .unwrap();
        runtime.catalog.install(&db, "example-plugin").unwrap();
        Self {
            root,
            runtime,
            package,
        }
    }
    fn close(self) {
        drop(self.runtime);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn portable_snapshot_is_deterministic_complete_and_independent_of_source() {
    let f = Fixture::new("snapshot");
    let snapshot = f
        .runtime
        .snapshot("example-plugin", "workspace-example")
        .await
        .unwrap();
    snapshot.validate().unwrap();
    assert_eq!(snapshot.manifest.files.len(), 5);
    assert!(!snapshot.manifest.has_tools);
    let encoded = serde_json::to_string(&snapshot.manifest).unwrap();
    assert!(!encoded.contains(f.root.to_str().unwrap()));
    assert!(!encoded.contains("mcp.json"));
    assert!(!encoded.contains("private-settings"));
    let document = std::str::from_utf8(&snapshot.resources[0].bytes).unwrap();
    assert!(document.contains("skills/example/SKILL.md"));
    assert!(!document.contains(f.root.to_str().unwrap()));
    let again = f
        .runtime
        .snapshot("example-plugin", "workspace-example")
        .await
        .unwrap();
    assert_eq!(snapshot.digest().unwrap(), again.digest().unwrap());
    let binary = snapshot
        .resources
        .iter()
        .find(|r| r.file.path.ends_with("example.bin"))
        .unwrap();
    assert_eq!(binary.bytes, [0, 128, 255]);
    std::fs::write(
        f.package.join("skills/example/scripts/example.py"),
        b"print('changed')\n",
    )
    .unwrap();
    let changed = f
        .runtime
        .snapshot("example-plugin", "workspace-example")
        .await
        .unwrap();
    assert_ne!(snapshot.digest().unwrap(), changed.digest().unwrap());
    assert!(
        snapshot
            .resources
            .iter()
            .any(|r| r.bytes == b"print('example')\n")
    );
    snapshot.validate().unwrap();
    let local = beale_tools::AgentFiles::new().unwrap();
    let (document, _) = f
        .runtime
        .publish("example-plugin", "workspace-example", &local)
        .await
        .unwrap();
    assert!(document.is_file());
    for resource in &changed.resources {
        assert_eq!(
            std::fs::read(
                local
                    .root()
                    .join("plugins/example-plugin")
                    .join(&resource.file.path)
            )
            .unwrap(),
            resource.bytes
        );
    }
    let local_root = local.root().to_owned();
    drop(local);
    std::fs::remove_dir_all(local_root).unwrap();
    let mut corrupt = changed;
    corrupt.resources[0].bytes.push(b'x');
    assert!(corrupt.validate().is_err());
    f.close();
}

#[tokio::test]
async fn portable_snapshot_rejects_oversized_resources_and_uninstalled_packages() {
    let f = Fixture::new("limits");
    let path = f.package.join("skills/example/assets/too-large.bin");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_RESOURCE_BYTES + 1).unwrap();
    drop(file);
    assert!(
        f.runtime
            .snapshot("example-plugin", "workspace-example")
            .await
            .is_err()
    );
    std::fs::remove_file(path).unwrap();
    f.runtime
        .db
        .set_extension_scope("plugin", "example-plugin", false, &[])
        .unwrap();
    assert!(
        f.runtime
            .snapshot("example-plugin", "workspace-example")
            .await
            .is_err()
    );
    f.runtime
        .db
        .set_extension_scope("plugin", "example-plugin", true, &[])
        .unwrap();
    let skill = f.package.join("skills/example/SKILL.md");
    std::fs::write(&skill, vec![b'x'; 64_001]).unwrap();
    assert!(
        f.runtime
            .snapshot("example-plugin", "workspace-example")
            .await
            .is_err()
    );
    std::fs::write(&skill, b"Synthetic repaired guidance").unwrap();
    assert!(
        f.runtime
            .snapshot("example-plugin", "workspace-example")
            .await
            .is_ok()
    );
    f.runtime.db.uninstall_plugin("example-plugin").unwrap();
    assert!(
        f.runtime
            .snapshot("example-plugin", "workspace-example")
            .await
            .is_err()
    );
    f.close();
}

#[cfg(unix)]
#[tokio::test]
async fn portable_snapshot_preserves_execute_bits_and_rejects_linked_skill_directories() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new("links");
    let script = f.package.join("skills/example/scripts/example.py");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let snapshot = f
        .runtime
        .snapshot("example-plugin", "workspace-example")
        .await
        .unwrap();
    assert!(
        snapshot
            .resources
            .iter()
            .find(|r| r.file.path.ends_with("example.py"))
            .unwrap()
            .file
            .executable
    );
    symlink(
        f.package.join("skills/example"),
        f.package.join("skills/linked"),
    )
    .unwrap();
    assert!(
        f.runtime
            .snapshot("example-plugin", "workspace-example")
            .await
            .is_err()
    );
    f.close();
}
