//! Local, process-scoped diagnostics. Never send these files to model tools.
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 4096;
const BACKUPS: usize = 3;
static LOGGER: OnceLock<Mutex<LogFile>> = OnceLock::new();

struct LogFile {
    path: PathBuf,
    file: Option<File>,
    len: u64,
}

impl LogFile {
    fn open(root: &Path, name: &str) -> io::Result<Self> {
        let dir = root.join("logs");
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        restrict_windows_acl(&dir, true)?;
        let path = dir.join(format!("{name}.log"));
        let file = open_private(&path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path,
            file: Some(file),
            len,
        })
    }

    fn write(&mut self, line: &[u8]) -> io::Result<()> {
        if self.len.saturating_add(line.len() as u64) > MAX_FILE_BYTES
            && let Err(error) = self.rotate()
        {
            self.file = Some(open_private(&self.path)?);
            self.len = self
                .file
                .as_ref()
                .map_or(0, |file| file.metadata().map_or(0, |meta| meta.len()));
            return Err(error);
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is closed"))?
            .write_all(line)?;
        self.len = self.len.saturating_add(line.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }
        // The active file must be closed before rename on Windows.
        self.file.take();
        for index in (1..=BACKUPS).rev() {
            let source = if index == 1 {
                self.path.clone()
            } else {
                backup_path(&self.path, index - 1)
            };
            let destination = backup_path(&self.path, index);
            if destination.exists() {
                fs::remove_file(&destination)?;
            }
            if source.exists() {
                fs::rename(source, destination)?;
            }
        }
        self.file = Some(open_private(&self.path)?);
        self.len = 0;
        Ok(())
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    path.with_extension(format!("log.{index}"))
}

fn open_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    restrict_windows_acl(path, false)?;
    Ok(file)
}

#[cfg(windows)]
fn restrict_windows_acl(path: &Path, directory: bool) -> io::Result<()> {
    let user =
        std::env::var("USERNAME").map_err(|_| io::Error::other("Windows user is unavailable"))?;
    let domain = std::env::var("USERDOMAIN")
        .map_err(|_| io::Error::other("Windows domain is unavailable"))?;
    let permission = if directory {
        format!("{domain}\\{user}:(OI)(CI)F")
    } else {
        format!("{domain}\\{user}:F")
    };
    let output = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(permission)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("cannot restrict Windows log ACL"));
    }
    Ok(())
}

pub fn host_root() -> io::Result<PathBuf> {
    if let Some(root) = std::env::var_os("BEALE_V2_HOME") {
        return Ok(PathBuf::from(root));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable"))?;
    Ok(PathBuf::from(home).join(".beale").join("v2"))
}

/// Initialize once at process startup. The log stays open for the process lifetime.
pub fn init(name: &str) -> io::Result<PathBuf> {
    if !matches!(name, "app-server" | "desktop") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid log name",
        ));
    }
    let writer = LogFile::open(&host_root()?, name)?;
    let path = writer.path.clone();
    LOGGER
        .set(Mutex::new(writer))
        .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "logger already initialized"))?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let location = panic.location().map_or("unknown", |at| at.file());
        let line = panic.location().map_or(0, |at| at.line());
        // Panic payloads can contain target data; record only the code location.
        if let Some(logger) = LOGGER.get()
            && let Ok(mut writer) = logger.try_lock()
        {
            let _ = writer.write(format!("[PANIC] at {location}:{line}\n").as_bytes());
        }
        previous(panic);
    }));
    Ok(path)
}

/// Write a bounded, single-line diagnostic to the local file and stderr.
pub fn emit(level: &str, source: &str, args: fmt::Arguments<'_>) {
    let message = sanitize(&args.to_string());
    let now: chrono::DateTime<chrono::Utc> = SystemTime::now().into();
    let line = format!(
        "{} [{level}] {source}: {message}\n",
        now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    if let Some(logger) = LOGGER.get()
        && let Ok(mut writer) = logger.lock()
        && let Err(error) = writer.write(line.as_bytes())
    {
        let _ = writeln!(io::stderr().lock(), "Beale log write failed: {error}");
    }
    let _ = io::stderr().lock().write_all(line.as_bytes());
}

fn sanitize(value: &str) -> String {
    let mut bounded = String::with_capacity(value.len().min(MAX_MESSAGE_BYTES));
    for character in value.chars() {
        let replacement = if character.is_control() {
            ' '
        } else {
            character
        };
        if bounded.len() + replacement.len_utf8() > MAX_MESSAGE_BYTES {
            bounded.push('…');
            break;
        }
        bounded.push(replacement);
    }
    bounded
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::emit("ERROR", module_path!(), format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::emit("INFO", module_path!(), format_args!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_bounded_private_logs_without_research_data() {
        let root = std::env::temp_dir().join(format!("beale-log-example-{}", std::process::id()));
        let mut writer = LogFile::open(&root, "desktop").unwrap();
        let first = b"synthetic first event\n";
        writer.write(first).unwrap();
        writer
            .write(&vec![b'x'; MAX_FILE_BYTES as usize - first.len()])
            .unwrap();
        writer.write(b"synthetic second event\n").unwrap();
        assert_eq!(
            fs::read_to_string(&writer.path).unwrap(),
            "synthetic second event\n"
        );
        let backup = fs::read(backup_path(&writer.path, 1)).unwrap();
        assert_eq!(backup.len() as u64, MAX_FILE_BYTES);
        assert!(backup.starts_with(first));
        drop(writer);
        fs::remove_file(root.join("logs/desktop.log")).unwrap();
        fs::remove_file(root.join("logs/desktop.log.1")).unwrap();
        fs::remove_dir(root.join("logs")).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn sanitizes_newlines_and_bounds_entries() {
        assert_eq!(sanitize("synthetic\nsecret"), "synthetic secret");
        assert!(sanitize(&"a".repeat(MAX_MESSAGE_BYTES + 10)).len() <= MAX_MESSAGE_BYTES + 3);
    }
}
