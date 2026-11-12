use anyhow::{Context, Result, ensure};
use beale_tools::AgentFiles;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

const NAME: &str = "SYSTEM.md";
const DEFAULT: &str = include_str!("../SYSTEM.default.md");
// Migration-only snapshots; never included in model prompts.
const LEGACY_DEFAULTS: &[&str] = &[
    include_str!("../SYSTEM.legacy.md"),
    include_str!("../SYSTEM.files.legacy.md"),
    include_str!("../SYSTEM.webfetch.legacy.md"),
];
const MAX_FILE_BYTES: usize = 65_536;
const MAX_COMPOSED_BYTES: usize = 96_000;

/// Keep extension discovery in a file; only its location belongs in the prompt.
pub(crate) fn extension_catalog(
    files: &AgentFiles,
    relative: &str,
    entries: &str,
) -> Result<String> {
    let entries = if entries.is_empty() {
        "None available.\n"
    } else {
        entries
    };
    let content =
        format!("# Extensions\n\nAvailable for this agent in this workspace:\n\n{entries}");
    let path = files.write(relative, content.as_bytes())?;
    Ok(format!(
        "Extension catalog: {}",
        json!(path.to_string_lossy())
    ))
}

/// Create the editable host prompt once. Existing user content is never replaced.
pub(crate) fn ensure_file(root: &Path) -> Result<()> {
    let path = root.join(NAME);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            if let Err(error) = file
                .write_all(DEFAULT.as_bytes())
                .and_then(|_| file.sync_all())
            {
                let _ = fs::remove_file(&path);
                return Err(error).context("initialize system prompt");
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            // Upgrade only the byte-for-byte stock prompt; preserve every user edit.
            let metadata = fs::symlink_metadata(&path).context("inspect system prompt")?;
            if metadata.file_type().is_file()
                && metadata.len() <= MAX_FILE_BYTES as u64
                && fs::read_to_string(&path).ok().is_some_and(|text| {
                    LEGACY_DEFAULTS
                        .iter()
                        .any(|legacy| text.replace("\r\n", "\n") == legacy.replace("\r\n", "\n"))
                })
            {
                let temporary = root.join(format!("system-{}.tmp", uuid::Uuid::new_v4()));
                let mut file = options
                    .open(&temporary)
                    .context("create updated system prompt")?;
                file.write_all(DEFAULT.as_bytes())
                    .context("write updated system prompt")?;
                file.sync_all().context("sync updated system prompt")?;
                drop(file);
                if let Err(error) = fs::rename(&temporary, &path) {
                    let _ = fs::remove_file(temporary);
                    return Err(error).context("replace stock system prompt");
                }
            }
            Ok(())
        }
        Err(error) => Err(error).context("create system prompt"),
    }
}

/// Load the current file for one turn, then append active runtime guidance verbatim.
pub(crate) fn load(root: &Path, runtime_sections: &[&str]) -> Result<String> {
    ensure_file(root)?;
    let path = root.join(NAME);
    let metadata = fs::symlink_metadata(&path).context("inspect system prompt")?;
    ensure!(
        metadata.file_type().is_file(),
        "system prompt must be a regular file"
    );
    ensure!(
        metadata.len() <= MAX_FILE_BYTES as u64,
        "system prompt is too large"
    );
    let bytes = fs::read(&path).context("read system prompt")?;
    ensure!(bytes.len() <= MAX_FILE_BYTES, "system prompt is too large");
    let mut prompt = String::from_utf8(bytes).context("system prompt is not UTF-8")?;
    ensure!(!prompt.trim().is_empty(), "system prompt is empty");
    for section in runtime_sections {
        if section.trim().is_empty() {
            continue;
        }
        if !prompt.ends_with('\n') {
            prompt.push('\n');
        }
        prompt.push('\n');
        prompt.push_str(section);
        ensure!(
            prompt.len() <= MAX_COMPOSED_BYTES,
            "composed system prompt is too large"
        );
    }
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn prompt_points_to_catalog_without_inlining_or_repeating_guidance() {
        let files = AgentFiles::new().unwrap();
        let entries = "- Feature `example-feature`: Example extension instructions.\n";
        let section = extension_catalog(&files, "EXTENSIONS.md", entries).unwrap();
        let path: String =
            serde_json::from_str(section.strip_prefix("Extension catalog: ").unwrap()).unwrap();
        assert!(fs::read_to_string(path).unwrap().contains(entries));
        let prompt = load(files.root(), &[&section]).unwrap();
        assert!(!prompt.contains("example-feature"));
        assert_eq!(prompt.matches("RULES.md").count(), 1);
        assert_eq!(prompt.matches("AGENTS.md").count(), 1);
        assert_eq!(prompt.matches("Base tools are").count(), 1);
        for removed in [
            "get_scope",
            "tool_result_page",
            "load_plugin_or_feature",
            "webfetch",
        ] {
            assert!(
                !prompt.contains(removed),
                "unexpected base prompt tool: {removed}"
            );
        }
        fs::remove_dir_all(files.root()).unwrap();
    }

    #[test]
    fn empty_extension_catalog_is_explicit() {
        let files = AgentFiles::new().unwrap();
        extension_catalog(&files, "subagents/EXTENSIONS.md", "").unwrap();
        assert!(
            fs::read_to_string(files.root().join("subagents/EXTENSIONS.md"))
                .unwrap()
                .ends_with("None available.\n")
        );
        fs::remove_dir_all(files.root()).unwrap();
    }

    #[test]
    fn upgrades_stock_prompt_without_replacing_custom_instructions() {
        let root =
            std::env::temp_dir().join(format!("beale-system-example-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for legacy in LEGACY_DEFAULTS {
            fs::write(
                root.join(NAME),
                legacy.replace("\r\n", "\n").replace('\n', "\r\n"),
            )
            .unwrap();
            ensure_file(&root).unwrap();
            assert_eq!(fs::read_to_string(root.join(NAME)).unwrap(), DEFAULT);
            let custom = format!("{legacy}\nExample operator instruction.\n");
            fs::write(root.join(NAME), &custom).unwrap();
            ensure_file(&root).unwrap();
            assert_eq!(fs::read_to_string(root.join(NAME)).unwrap(), custom);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initializes_once_and_reloads_user_edits_without_substitution() {
        let root = std::env::temp_dir().join(format!("beale-system-example-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        ensure_file(&root).unwrap();
        assert_eq!(fs::read_to_string(root.join(NAME)).unwrap(), DEFAULT);
        fs::write(root.join(NAME), "# Example\n\nLiteral {{workspace}} text.").unwrap();
        let prompt = load(&root, &["## Active tools\n\nRead files."]).unwrap();
        assert_eq!(
            prompt,
            "# Example\n\nLiteral {{workspace}} text.\n\n## Active tools\n\nRead files."
        );
        assert_eq!(
            fs::read_to_string(root.join(NAME)).unwrap(),
            "# Example\n\nLiteral {{workspace}} text."
        );
        fs::write(root.join(NAME), "# Updated").unwrap();
        assert_eq!(load(&root, &[]).unwrap(), "# Updated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_empty_or_oversized_prompt() {
        let root = std::env::temp_dir().join(format!("beale-system-example-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(NAME), "  ").unwrap();
        assert!(load(&root, &[]).is_err());
        fs::write(root.join(NAME), "x".repeat(MAX_FILE_BYTES + 1)).unwrap();
        assert!(load(&root, &[]).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
