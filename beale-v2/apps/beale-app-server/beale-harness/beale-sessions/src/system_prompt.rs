use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

const NAME: &str = "SYSTEM.md";
const DEFAULT: &str = include_str!("../SYSTEM.default.md");
const MAX_FILE_BYTES: usize = 65_536;
const MAX_COMPOSED_BYTES: usize = 96_000;

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
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
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
