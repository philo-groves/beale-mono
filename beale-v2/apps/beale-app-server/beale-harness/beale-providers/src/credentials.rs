use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub client_id: String,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

pub struct CredentialStore {
    root: PathBuf,
}

impl CredentialStore {
    pub fn new() -> Result<Self> {
        let root = if let Some(path) = std::env::var_os("BEALE_V2_HOME") {
            PathBuf::from(path)
        } else {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .context("home directory is unavailable")?;
            PathBuf::from(home).join(".beale").join("v2")
        };
        Ok(Self { root })
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn control_token(&self) -> Result<String> {
        let path = self.root.join("app-server-control-token");
        match std::fs::read_to_string(&path) {
            Ok(token)
                if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                return Ok(token);
            }
            Ok(_) => bail!("invalid saved app-server control token"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("cannot read app-server control token"),
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.write_private(&path, token.as_bytes())?;
        Ok(token)
    }

    pub fn api_key_configured(&self, provider: &str) -> Result<bool> {
        Ok(self.api_key_path(provider)?.exists())
    }

    pub fn load_api_key(&self, provider: &str) -> Result<Option<String>> {
        match std::fs::read(self.api_key_path(provider)?) {
            Ok(bytes) => {
                let value = String::from_utf8(unprotect_api_key(&bytes)?)
                    .context("invalid saved API key")?;
                Ok(Some(value))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("cannot read saved API key"),
        }
    }

    pub fn save_api_key(&self, provider: &str, key: &str) -> Result<()> {
        let key = key.trim();
        if key.is_empty() || key.len() > 8192 || key.chars().any(char::is_control) {
            bail!("invalid API key");
        }
        self.write_private(
            &self.api_key_path(provider)?,
            &protect_api_key(key.as_bytes())?,
        )
    }

    pub fn remove_api_key(&self, provider: &str) -> Result<()> {
        remove_if_exists(&self.api_key_path(provider)?)
    }

    pub fn remove_oauth(&self, provider: &str) -> Result<()> {
        remove_if_exists(&self.credential_path(provider)?)
    }

    pub fn host_id(&self) -> Result<String> {
        let path = self.root.join("host-id");
        match std::fs::read_to_string(&path) {
            Ok(id) => {
                let id = id.trim();
                if id.starts_with("urn:uuid:") && uuid::Uuid::parse_str(&id[9..]).is_ok() {
                    return Ok(id.to_owned());
                }
                bail!("invalid persisted host identifier");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("cannot read persisted host identifier"),
        }
        let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        self.write_private(&path, id.as_bytes())?;
        Ok(id)
    }

    pub fn load(&self, provider: &str) -> Result<Option<OAuthCredential>> {
        let path = self.credential_path(provider)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("invalid saved credentials")?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("cannot read saved credentials"),
        }
    }

    pub fn save(&self, provider: &str, credential: &OAuthCredential) -> Result<()> {
        let path = self.credential_path(provider)?;
        self.write_private(&path, &serde_json::to_vec(credential)?)
    }

    pub(crate) async fn lock(&self, provider: &str) -> Result<std::fs::File> {
        let path = self.credential_path(provider)?.with_extension("lock");
        self.ensure_private_root()?;
        tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(&path).context("cannot open credential lock")?;
            #[cfg(windows)]
            restrict_windows_acl(&path)?;
            file.lock().context("cannot lock credentials")?;
            Ok(file)
        })
        .await
        .context("credential lock task failed")?
    }

    fn credential_path(&self, provider: &str) -> Result<PathBuf> {
        match provider {
            "openai" | "xai" => Ok(self.root.join(format!("{provider}-oauth.json"))),
            _ => bail!("unknown provider"),
        }
    }

    fn api_key_path(&self, provider: &str) -> Result<PathBuf> {
        match provider {
            "openai" | "xai" => Ok(self.root.join(format!("{provider}-api-key"))),
            _ => bail!("unknown provider"),
        }
    }

    fn write_private(&self, path: &Path, contents: &[u8]) -> Result<()> {
        self.ensure_private_root()?;
        let temporary = self.root.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| -> Result<()> {
            use std::io::Write;
            let mut file = options
                .open(&temporary)
                .context("cannot create private credential file")?;
            file.write_all(contents)?;
            file.sync_all()?;
            #[cfg(windows)]
            restrict_windows_acl(&temporary)?;
            std::fs::rename(&temporary, path).context("cannot replace saved credential")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    fn ensure_private_root(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root).context("cannot create host data directory")?;
        #[cfg(windows)]
        restrict_windows_acl(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))
                .context("cannot restrict host data directory")?;
        }
        Ok(())
    }
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("cannot remove credential"),
    }
}

#[cfg(windows)]
fn protect_api_key(bytes: &[u8]) -> Result<Vec<u8>> {
    dpapi(bytes, true)
}

#[cfg(windows)]
fn unprotect_api_key(bytes: &[u8]) -> Result<Vec<u8>> {
    dpapi(bytes, false)
}

#[cfg(windows)]
fn dpapi(bytes: &[u8], protect: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len())?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // DPAPI copies input into a LocalAlloc output buffer. We copy it before LocalFree.
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error())
            .context("Windows credential protection failed");
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

#[cfg(not(windows))]
fn protect_api_key(bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(bytes.to_vec())
}

#[cfg(not(windows))]
fn unprotect_api_key(bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(bytes.to_vec())
}

#[cfg(windows)]
fn restrict_windows_acl(path: &Path) -> Result<()> {
    use std::process::Command;
    let user = std::env::var("USERNAME").context("Windows user is unavailable")?;
    let domain = std::env::var("USERDOMAIN").context("Windows domain is unavailable")?;
    let principal = format!("{domain}\\{user}");
    let status = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{principal}:F"))
        .output()
        .context("cannot set private Windows ACL")?;
    if !status.status.success() {
        bail!("cannot set private Windows ACL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_credential_name() {
        let store = CredentialStore::at(std::env::temp_dir());
        assert!(store.load("../outside").is_err());
    }
}
