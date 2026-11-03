use anyhow::{Context, Result, bail};
use beale_storage::HostDatabase;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    db: Mutex<Option<HostDatabase>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ModelDefaults {
    pub lead: String,
    pub small: String,
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
        Ok(Self::at(root))
    }

    pub fn at(root: PathBuf) -> Self {
        Self {
            root,
            db: Mutex::new(None),
        }
    }

    fn db(&self) -> Result<HostDatabase> {
        let mut cache = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("provider database lock poisoned"))?;
        if let Some(db) = cache.as_ref() {
            return Ok(db.clone());
        }
        let db = HostDatabase::open(&self.root)?;
        *cache = Some(db.clone());
        Ok(db)
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

    pub fn load_lead_provider(&self) -> Result<Option<String>> {
        match self.db()?.setting("lead-provider")? {
            Some(value) if value == "openai" || value == "xai" => Ok(Some(value)),
            Some(_) => bail!("invalid saved lead provider"),
            None => Ok(None),
        }
    }

    pub fn save_lead_provider(&self, provider: &str) -> Result<()> {
        match provider {
            "openai" | "xai" => self.db()?.save_setting("lead-provider", Some(provider)),
            _ => bail!("unknown lead provider"),
        }
    }

    pub fn clear_lead_provider(&self) -> Result<()> {
        self.db()?.save_setting("lead-provider", None)
    }

    pub fn source_priority(&self, provider: &str) -> Result<&'static str> {
        let path = self.source_priority_path(provider)?;
        match self.db()?.setting(setting_name(&path)?)? {
            Some(value) if value == "api-key" => Ok("api-key"),
            Some(_) => bail!("invalid saved source priority"),
            None => Ok("subscription"),
        }
    }

    pub fn save_source_priority(&self, provider: &str, mode: &str) -> Result<()> {
        let path = self.source_priority_path(provider)?;
        let key = setting_name(&path)?;
        match mode {
            "subscription" => self.db()?.save_setting(key, None),
            "api-key" => self.db()?.save_setting(key, Some("api-key")),
            _ => bail!("unknown source priority"),
        }
    }

    pub fn daybreak_mode(&self) -> Result<&'static str> {
        match self.db()?.setting("openai-daybreak-mode")? {
            Some(value) if value == "red" => Ok("red"),
            Some(_) => bail!("invalid saved Daybreak mode"),
            None => Ok("blue"),
        }
    }

    pub fn save_daybreak_mode(&self, mode: &str) -> Result<()> {
        match mode {
            "blue" => self.db()?.save_setting("openai-daybreak-mode", None),
            "red" => self.db()?.save_setting("openai-daybreak-mode", Some("red")),
            _ => bail!("unknown Daybreak mode"),
        }
    }

    pub fn load_model_defaults(&self, provider: &str, mode: &str) -> Result<Option<ModelDefaults>> {
        let path = self.model_defaults_path(provider, mode)?;
        match self.db()?.setting(setting_name(&path)?)? {
            Some(value) => Ok(Some(
                serde_json::from_str(&value).context("invalid saved model defaults")?,
            )),
            None => Ok(None),
        }
    }

    pub fn save_model_defaults(
        &self,
        provider: &str,
        mode: &str,
        defaults: &ModelDefaults,
    ) -> Result<()> {
        if !crate::models::valid_id(&defaults.lead) || !crate::models::valid_id(&defaults.small) {
            bail!("invalid model defaults");
        }
        let path = self.model_defaults_path(provider, mode)?;
        self.db()?.save_setting(
            setting_name(&path)?,
            Some(&serde_json::to_string(defaults)?),
        )
    }

    pub fn host_id(&self) -> Result<String> {
        if let Some(id) = self.db()?.setting("host-id")? {
            let id = id.trim();
            if id.starts_with("urn:uuid:") && uuid::Uuid::parse_str(&id[9..]).is_ok() {
                return Ok(id.to_owned());
            }
            bail!("invalid persisted host identifier");
        }
        let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        self.db()?.save_setting("host-id", Some(&id))?;
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
            restrict_windows_acl(&path, false)?;
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

    fn source_priority_path(&self, provider: &str) -> Result<PathBuf> {
        match provider {
            "openai" | "xai" => Ok(self.root.join(format!("{provider}-source-priority"))),
            _ => bail!("unknown provider"),
        }
    }

    fn model_defaults_path(&self, provider: &str, mode: &str) -> Result<PathBuf> {
        match (provider, mode) {
            ("openai" | "xai", "api-key" | "subscription") => {
                Ok(self.root.join(format!("{provider}-{mode}-models.json")))
            }
            _ => bail!("unknown provider model scope"),
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
            restrict_windows_acl(&temporary, false)?;
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
        restrict_windows_acl(&self.root, true)?;
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

fn setting_name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .context("invalid provider setting name")
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
fn restrict_windows_acl(path: &Path, directory: bool) -> Result<()> {
    use std::process::Command;
    let user = std::env::var("USERNAME").context("Windows user is unavailable")?;
    let domain = std::env::var("USERDOMAIN").context("Windows domain is unavailable")?;
    let principal = format!("{domain}\\{user}");
    let permission = if directory {
        format!("{principal}:(OI)(CI)F")
    } else {
        format!("{principal}:F")
    };
    let status = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(permission)
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

    #[test]
    fn model_defaults_are_scoped_by_provider_and_method() {
        let root = std::env::temp_dir().join(format!("beale-model-test-{}", uuid::Uuid::new_v4()));
        let store = CredentialStore::at(root.clone());
        let defaults = ModelDefaults {
            lead: "example-lead".to_owned(),
            small: "example-small".to_owned(),
        };
        store
            .save_model_defaults("openai", "subscription", &defaults)
            .unwrap();
        let loaded = store
            .load_model_defaults("openai", "subscription")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.lead, defaults.lead);
        assert_eq!(loaded.small, defaults.small);
        assert!(
            store
                .load_model_defaults("openai", "api-key")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .load_model_defaults("xai", "subscription")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .save_model_defaults("../other", "subscription", &defaults)
                .is_err()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_priority_defaults_to_subscription_and_is_provider_scoped() {
        let root =
            std::env::temp_dir().join(format!("beale-priority-test-{}", uuid::Uuid::new_v4()));
        let store = CredentialStore::at(root.clone());
        assert_eq!(store.source_priority("openai").unwrap(), "subscription");
        store.save_source_priority("openai", "api-key").unwrap();
        assert_eq!(store.source_priority("openai").unwrap(), "api-key");
        assert_eq!(store.source_priority("xai").unwrap(), "subscription");
        assert!(store.save_source_priority("../other", "api-key").is_err());
        assert!(store.save_source_priority("openai", "unknown").is_err());
        store
            .save_source_priority("openai", "subscription")
            .unwrap();
        assert_eq!(store.source_priority("openai").unwrap(), "subscription");
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn daybreak_mode_defaults_to_blue_and_persists_red() {
        let root =
            std::env::temp_dir().join(format!("beale-daybreak-example-{}", uuid::Uuid::new_v4()));
        let store = CredentialStore::at(root.clone());
        assert_eq!(store.daybreak_mode().unwrap(), "blue");
        store.save_daybreak_mode("red").unwrap();
        assert_eq!(store.daybreak_mode().unwrap(), "red");
        assert!(store.save_daybreak_mode("unknown").is_err());
        store.save_daybreak_mode("blue").unwrap();
        assert_eq!(store.daybreak_mode().unwrap(), "blue");
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn database_settings_ignore_old_files_and_survive_restart() {
        let root = std::env::temp_dir().join(format!(
            "beale-provider-migration-example-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("lead-provider"), b"xai").unwrap();
        std::fs::write(root.join("openai-source-priority"), b"api-key").unwrap();
        let store = CredentialStore::at(root.clone());
        assert!(store.load_lead_provider().unwrap().is_none());
        assert_eq!(store.source_priority("openai").unwrap(), "subscription");
        store.save_lead_provider("openai").unwrap();
        drop(store);
        std::fs::write(root.join("lead-provider"), b"invalid stale value").unwrap();
        let reopened = CredentialStore::at(root.clone());
        assert_eq!(
            reopened.load_lead_provider().unwrap().as_deref(),
            Some("openai")
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
