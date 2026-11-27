use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

#[derive(Clone, Debug)]
pub struct SnapshotInput {
    pub source: PathBuf,
    /// Empty for the primary workspace; explicit relative mount for external inputs.
    pub destination: String,
}

#[derive(Clone, Debug)]
pub struct Destination {
    pub case_sensitive: bool,
    pub windows_names: bool,
    pub symlinks: bool,
    pub posix_permissions: bool,
    pub require_executable_metadata: bool,
    pub executable_bits: bool,
}
impl Destination {
    pub fn portable() -> Self {
        Self {
            case_sensitive: true,
            windows_names: false,
            symlinks: true,
            posix_permissions: true,
            require_executable_metadata: false,
            executable_bits: true,
        }
    }
    pub fn native() -> Self {
        Self {
            case_sensitive: cfg!(unix) && !cfg!(target_os = "macos"),
            windows_names: cfg!(windows),
            symlinks: cfg!(unix),
            posix_permissions: cfg!(unix),
            require_executable_metadata: false,
            executable_bits: cfg!(unix),
        }
    }
    /// Simulated destinations validate the same manifest before accepting any bytes.
    pub fn validate(&self, manifest: &Manifest) -> Result<()> {
        validate(manifest, self)
    }
}

pub(super) fn excluded_path(path: &str) -> bool {
    path.split('/').any(|part| {
        let p = part.to_ascii_lowercase();
        matches!(
            p.as_str(),
            ".beale"
                | ".codex"
                | ".aws"
                | ".ssh"
                | ".gnupg"
                | ".git"
                | ".gitconfig"
                | ".git-credentials"
                | ".netrc"
                | ".npmrc"
                | "credentials.json"
                | "auth.json"
        ) || p == ".env"
            || p.starts_with(".env.")
            || ["memory.sqlite", "memories.sqlite", "state.sqlite"]
                .iter()
                .any(|name| p == *name || p.starts_with(&format!("{name}-")))
    })
}

fn supported_name(path: &str) -> bool {
    path.split('/').all(|p| {
        let base = p.split('.').next().unwrap_or_default().to_ascii_uppercase();
        !p.ends_with(['.', ' '])
            && !p.contains(['<', '>', '"', '|', '?', '*'])
            && !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !(base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
    })
}

fn link_destination(path: &str, target: &str) -> Result<String> {
    ensure!(
        !target.is_empty() && !target.starts_with('/') && !target.contains(['\\', ':', '\0']),
        "unsupported link at {path}"
    );
    let mut parts: Vec<_> = path.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                ensure!(parts.pop().is_some(), "escaping link at {path}");
            }
            component => parts.push(component),
        }
    }
    Ok(parts.join("/"))
}

pub(super) fn validate(manifest: &Manifest, destination: &Destination) -> Result<()> {
    ensure!(manifest.version == VERSION, "unsupported manifest version");
    let mut names = BTreeMap::new();
    let paths: BTreeSet<_> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
    for entry in &manifest.entries {
        ensure!(
            relative_path(&entry.path) && !excluded_path(&entry.path),
            "invalid or excluded snapshot path: {}",
            entry.path
        );
        ensure!(
            !destination.windows_names || supported_name(&entry.path),
            "unsupported filename: {}",
            entry.path
        );
        let key = if destination.case_sensitive {
            entry.path.clone()
        } else {
            entry.path.to_lowercase()
        };
        ensure!(
            names.insert(key, &entry.kind).is_none(),
            "colliding snapshot path: {}",
            entry.path
        );
        let mut parent = entry.path.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            ensure!(
                manifest
                    .entries
                    .iter()
                    .any(|e| e.path == prefix && matches!(e.kind, EntryKind::Directory { .. })),
                "missing or non-directory parent: {}",
                entry.path
            );
            parent = prefix;
        }
        match &entry.kind {
            EntryKind::File {
                sha256,
                executable,
                mode,
                ..
            } => {
                ensure!(digest(sha256), "invalid digest: {}", entry.path);
                ensure!(
                    !destination.require_executable_metadata || executable.is_some(),
                    "missing executable metadata: {}",
                    entry.path
                );
                if !destination.executable_bits
                    && let Some(executable) = executable
                {
                    let indexed = manifest
                        .repositories
                        .iter()
                        .any(|r| r.executable_files.contains(&entry.path));
                    ensure!(
                        *executable == indexed,
                        "destination cannot preserve executable metadata: {}",
                        entry.path
                    );
                }
                if let Some(mode) = mode {
                    ensure!(
                        *mode <= 0o777
                            && executable.is_none_or(|exec| exec == (*mode & 0o111 != 0)),
                        "unsupported permission bits: {}",
                        entry.path
                    );
                    ensure!(
                        destination.posix_permissions || matches!(mode, 0o644 | 0o755),
                        "required permissions cannot be represented: {}",
                        entry.path
                    );
                }
            }
            EntryKind::Symlink { target, .. } => {
                ensure!(
                    destination.symlinks,
                    "destination cannot represent symlink: {}",
                    entry.path
                );
                let target = link_destination(&entry.path, target)?;
                ensure!(
                    paths.contains(target.as_str()),
                    "undeclared or excluded link target: {}",
                    entry.path
                );
                // Chained links are intentionally unsupported, avoiding cycles and ambiguous relocation.
                ensure!(
                    !manifest
                        .entries
                        .iter()
                        .any(|e| e.path == target && matches!(e.kind, EntryKind::Symlink { .. })),
                    "chained symlink unsupported: {}",
                    entry.path
                );
            }
            EntryKind::Directory { mode } => {
                if let Some(mode) = mode {
                    ensure!(
                        *mode <= 0o777 && (destination.posix_permissions || *mode == 0o755),
                        "required directory permissions cannot be represented: {}",
                        entry.path
                    );
                }
            }
        }
    }
    let mut repositories = BTreeSet::new();
    for repo in &manifest.repositories {
        ensure!(
            repo.path.is_empty() || relative_path(&repo.path),
            "invalid repository path"
        );
        ensure!(
            repo.path.is_empty()
                || manifest
                    .entries
                    .iter()
                    .any(|e| e.path == repo.path && matches!(e.kind, EntryKind::Directory { .. })),
            "missing repository directory"
        );
        ensure!(
            repositories.insert(&repo.path) && digest(&repo.bundle),
            "invalid repository manifest"
        );
        ensure!(
            matches!(repo.head.len(), 40 | 64) && repo.head.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid repository HEAD"
        );
        if let Some(branch) = &repo.branch {
            ensure!(
                branch.starts_with("refs/heads/") && relative_path(branch),
                "invalid repository branch"
            );
        }
        ensure!(
            repo.executable_files.iter().all(|p| relative_path(p)
                && !excluded_path(p)
                && (repo.path.is_empty() || p.starts_with(&format!("{}/", repo.path)))),
            "invalid repository executable paths"
        );
    }
    Ok(())
}

pub(super) fn executable(metadata: &fs::Metadata) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}
fn mode(metadata: &fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}
pub(super) fn set_mode(path: &Path, executable: Option<bool>, mode: Option<u32>) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = mode.or_else(|| executable.map(|x| if x { 0o755 } else { 0o644 })) {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
            // Some mounted filesystems accept chmod but silently ignore it.
            // Never publish a file whose requested metadata was not preserved.
            ensure!(
                fs::metadata(path)?.permissions().mode() & 0o7777 == mode,
                "destination cannot preserve requested permission bits"
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, executable, mode);
    }
    Ok(())
}

/// Reject symlink ancestors as well as escaping paths, including for not-yet-created outputs.
pub(super) fn safe_destination(root: &Path, relative: &str) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(root)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "workspace root must not be a link"
    );
    ensure!(
        relative_path(relative) && !excluded_path(relative),
        "invalid or excluded file path: {relative}"
    );
    ensure!(
        !cfg!(windows) || supported_name(relative),
        "unsupported filename: {relative}"
    );
    let mut path = root.canonicalize()?;
    for component in relative.split('/') {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "linked file path rejected: {relative}"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}
pub(super) fn safe_file(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = safe_destination(root, relative)?;
    ensure!(
        path.is_file(),
        "attachment is not a regular file: {relative}"
    );
    Ok(path)
}

pub(super) fn publication_executable(
    workspace: &Path,
    source: &Path,
    metadata: &fs::Metadata,
) -> Result<Option<bool>> {
    if let Some(executable) = executable(metadata) {
        return Ok(Some(executable));
    }
    let workspace = workspace.canonicalize()?;
    for ancestor in source
        .ancestors()
        .skip(1)
        .take_while(|p| p.starts_with(&workspace))
    {
        if ancestor.join(".git").exists() {
            let relative = source
                .strip_prefix(ancestor)?
                .to_str()
                .context("non-UTF8 attachment path")?
                .replace('\\', "/");
            let index = git(ancestor, &["ls-files", "--stage", "-z", "--", &relative])?;
            return Ok(if index.starts_with(b"100755 ") {
                Some(true)
            } else if index.starts_with(b"100644 ") {
                Some(false)
            } else {
                None
            });
        }
    }
    Ok(None)
}

pub(super) fn flush(root: &Path) -> Result<()> {
    for child in fs::read_dir(root)? {
        let child = child?;
        let metadata = fs::symlink_metadata(child.path())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            flush(&child.path())?;
        } else if metadata.is_file() {
            #[cfg(unix)]
            std::fs::File::open(child.path())?.sync_all()?;
            #[cfg(windows)]
            #[allow(clippy::permissions_set_readonly_false)]
            // Windows attribute only; Unix uses a read handle above.
            {
                // FlushFileBuffers requires write access. Git objects may be read-only;
                // restore their exact attribute even when the flush fails.
                let permissions = metadata.permissions();
                if permissions.readonly() {
                    let mut writable = permissions.clone();
                    writable.set_readonly(false);
                    fs::set_permissions(child.path(), writable)?;
                }
                let flushed = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(child.path())
                    .and_then(|file| file.sync_all());
                if permissions.readonly() {
                    fs::set_permissions(child.path(), permissions)?;
                }
                flushed?;
            }
        } else {
            bail!("cannot checkpoint unsupported filesystem entry");
        }
    }
    sync_directory(root)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    // No caller-controlled Git overrides, hooks, credential helpers or global configuration.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .current_dir(std::env::temp_dir())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args([
            "-c",
            "core.longpaths=true",
            "-c",
            "core.hooksPath=",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "credential.helper=",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(["-C", &git_path(root)?])
        .args(args)
        .output()
        .context("Git is required to preserve repository inputs")?;
    ensure!(
        output.status.success(),
        "repository metadata unsupported or changed ({}): {}",
        args.first().unwrap_or(&"git"),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn git_path(path: &Path) -> Result<String> {
    let path = path.to_str().context("non-UTF8 Git staging path")?;
    // Git for Windows does not accept Win32 verbatim prefixes as revision/file arguments.
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        return Ok(format!("//{}", path.replace('\\', "/")));
    }
    Ok(path
        .strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('\\', "/"))
}

impl ResearchFiles {
    pub(super) fn capture_inputs(&self, inputs: &[SnapshotInput]) -> Result<Manifest> {
        ensure!(!inputs.is_empty(), "no snapshot inputs");
        let mut manifest = Manifest {
            version: VERSION,
            entries: vec![],
            repositories: vec![],
        };
        let mut mounts = BTreeSet::new();
        for input in inputs {
            ensure!(
                input.destination.is_empty() || relative_path(&input.destination),
                "invalid external input mount"
            );
            ensure!(
                mounts.insert(input.destination.clone()),
                "duplicate input mount"
            );
            let root = input.source.canonicalize()?;
            ensure!(
                root.is_dir()
                    && !root.starts_with(&self.host_root)
                    && !self.host_root.starts_with(&root),
                "invalid snapshot source"
            );
            if !input.destination.is_empty() {
                let mut prefix = String::new();
                for part in input.destination.split('/') {
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(part);
                    if !manifest.entries.iter().any(|e| e.path == prefix) {
                        manifest.entries.push(Entry {
                            path: prefix.clone(),
                            kind: EntryKind::Directory { mode: None },
                        });
                    }
                }
            }
            self.walk(&root, &input.destination, &mut manifest, &BTreeMap::new())?;
        }
        manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
        manifest.repositories.sort_by(|a, b| a.path.cmp(&b.path));
        validate(&manifest, &Destination::portable())?;
        let source_for = |path: &str| -> Result<PathBuf> {
            let input = inputs
                .iter()
                .filter(|i| {
                    i.destination.is_empty()
                        || path == i.destination
                        || path.starts_with(&format!("{}/", i.destination))
                })
                .max_by_key(|i| i.destination.len())
                .context("undeclared external input")?;
            let relative = if input.destination.is_empty() {
                path
            } else {
                path.strip_prefix(&input.destination)
                    .context("invalid mount")?
                    .trim_start_matches('/')
            };
            Ok(input.source.canonicalize()?.join(relative))
        };
        for entry in &manifest.entries {
            if let EntryKind::Symlink { target, .. } = &entry.kind {
                let source = source_for(&entry.path)?;
                let actual = source
                    .parent()
                    .context("missing link parent")?
                    .join(target)
                    .canonicalize()
                    .with_context(|| format!("undeclared link target: {}", entry.path))?;
                let expected =
                    source_for(&link_destination(&entry.path, target)?)?.canonicalize()?;
                ensure!(
                    actual == expected,
                    "link relocation would change target: {}",
                    entry.path
                );
            }
        }
        Ok(manifest)
    }

    fn walk(
        &self,
        root: &Path,
        prefix: &str,
        manifest: &mut Manifest,
        inherited: &BTreeMap<String, String>,
    ) -> Result<()> {
        ensure!(
            !root.canonicalize()?.starts_with(&self.host_root),
            "host runtime storage cannot be a snapshot input: {prefix}"
        );
        let mut modes = inherited.clone();
        if root.join(".git").exists() {
            // Staged changes and submodules need semantics beyond a tree plus a bundle.
            git(root, &["diff", "--cached", "--quiet", "--no-ext-diff"])?;
            // Exclusion also applies to history, not merely the checked-out tree.
            let history = git(
                root,
                &[
                    "log",
                    "--all",
                    "HEAD",
                    "--format=",
                    "--name-only",
                    "-z",
                    "--no-renames",
                    "--diff-merges=first-parent",
                    "--root",
                ],
            )?;
            for path in history.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                let path = std::str::from_utf8(path)?.trim_start_matches('\n');
                ensure!(
                    !excluded_path(path),
                    "repository history contains excluded path: {path}"
                );
            }
            let index = git(root, &["ls-files", "--stage", "-z"])?;
            let mut executable_files = Vec::new();
            for item in index.split(|b| *b == 0).filter(|x| !x.is_empty()) {
                let item = std::str::from_utf8(item).context("non-UTF8 repository path")?;
                let (metadata, path) = item.split_once('\t').context("invalid Git index")?;
                let parts: Vec<_> = metadata.split_whitespace().collect();
                ensure!(
                    parts.len() == 3 && parts[2] == "0" && parts[0] != "160000",
                    "unmerged index or submodule unsupported: {path}"
                );
                let path = if prefix.is_empty() {
                    path.into()
                } else {
                    format!("{prefix}/{path}")
                };
                if parts[0] == "100755" {
                    executable_files.push(path.clone());
                }
                modes.insert(path, parts[0].into());
            }
            let head = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?
                .trim()
                .to_owned();
            let branch =
                String::from_utf8(git(root, &["rev-parse", "--symbolic-full-name", "HEAD"])?)?
                    .trim()
                    .to_owned();
            let branch = if branch == "HEAD" { None } else { Some(branch) };
            let bundle = self
                .root
                .join("staging")
                .join(format!("{}.bundle", Uuid::new_v4()));
            git(
                root,
                &["bundle", "create", &git_path(&bundle)?, "--all", "HEAD"],
            )?;
            let (digest, size) = self.put_file_blob(&bundle)?;
            fs::remove_file(bundle)?;
            manifest.repositories.push(Repository {
                path: prefix.into(),
                bundle: digest,
                size,
                head,
                branch,
                executable_files,
            });
        }
        let mut children = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF8 filename at {prefix}"))?;
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if excluded_path(&path) {
                continue;
            }
            let before = fs::symlink_metadata(child.path())?;
            let kind = if before.file_type().is_symlink()
                || modes.get(&path).is_some_and(|m| m == "120000")
            {
                let target = if before.file_type().is_symlink() {
                    fs::read_link(child.path())?
                        .to_str()
                        .context("non-UTF8 symlink")?
                        .replace('\\', "/")
                } else {
                    String::from_utf8(fs::read(child.path())?)?
                };
                link_destination(&path, &target)?;
                // Junctions, absolute pointers and directory symlinks outside declared roots fail validation.
                EntryKind::Symlink {
                    target,
                    directory: child.path().is_dir(),
                }
            } else if before.is_dir() {
                manifest.entries.push(Entry {
                    path: path.clone(),
                    kind: EntryKind::Directory {
                        mode: mode(&before),
                    },
                });
                self.walk(&child.path(), &path, manifest, &modes)?;
                continue;
            } else {
                ensure!(before.is_file(), "unsupported file type: {path}");
                let (sha256, size) = self.put_file_blob(&child.path())?;
                let after = fs::symlink_metadata(child.path())?;
                ensure!(
                    before.len() == size
                        && before.modified()? == after.modified()?
                        && before.permissions() == after.permissions()
                        && !after.file_type().is_symlink(),
                    "file changed during capture: {path}"
                );
                let exec = executable(&before).or_else(|| modes.get(&path).map(|m| m == "100755"));
                let mode = mode(&before);
                EntryKind::File {
                    size,
                    sha256,
                    executable: exec,
                    mode,
                }
            };
            manifest.entries.push(Entry { path, kind });
        }
        Ok(())
    }

    pub(super) fn write_manifest(&self, manifest: &Manifest, root: &Path) -> Result<()> {
        render_manifest(manifest, root, |digest, size| {
            self.verify_blob(digest, size)
        })
    }
}

pub(super) fn render_manifest(
    manifest: &Manifest,
    root: &Path,
    blob: impl Fn(&str, u64) -> Result<PathBuf>,
) -> Result<()> {
    // Entry order is not part of the wire contract. Parents must be created
    // before children, and permissions applied in the opposite order.
    let mut entries: Vec<_> = manifest.entries.iter().collect();
    entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    for entry in &entries {
        let path = root.join(&entry.path);
        match &entry.kind {
            EntryKind::Directory { .. } => fs::create_dir_all(path)?,
            EntryKind::File {
                size,
                sha256,
                executable,
                mode,
            } => {
                let source = blob(sha256, *size)?;
                let mut input = fs::File::open(source)?;
                let mut output = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                drop(output);
                ensure!(
                    file_hash(&path)? == (sha256.clone(), *size),
                    "materialized snapshot integrity failure"
                );
                set_mode(&path, *executable, *mode)?;
            }
            EntryKind::Symlink {
                target,
                directory: _,
            } => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, path)?;
                #[cfg(not(unix))]
                {
                    let _ = (target, path);
                    bail!("native symlinks unsupported");
                }
            }
        }
    }
    for repo in &manifest.repositories {
        let repository = root.join(&repo.path);
        let bundle = blob(&repo.bundle, repo.size)?;
        git(&repository, &["init", "--quiet", "--template="])?;
        if cfg!(windows) {
            git(&repository, &["config", "core.longpaths", "true"])?;
        }
        git(
            &repository,
            &[
                "fetch",
                "--quiet",
                "--update-head-ok",
                "--no-write-fetch-head",
                &git_path(&bundle)?,
                "+refs/*:refs/*",
                &repo.head,
            ],
        )?;
        git(
            &repository,
            &["update-ref", "--no-deref", "HEAD", &repo.head],
        )?;
        if let Some(branch) = &repo.branch {
            git(&repository, &["symbolic-ref", "HEAD", branch])?;
        }
        git(&repository, &["read-tree", &repo.head])?;
        // No remote, original worktree pointer, hooks, alternate or source config survives.
    }
    for entry in entries.iter().rev() {
        if let EntryKind::Directory { mode } = entry.kind {
            set_mode(&root.join(&entry.path), None, mode)?;
        }
    }
    sync_directory(root)?;
    Ok(())
}
