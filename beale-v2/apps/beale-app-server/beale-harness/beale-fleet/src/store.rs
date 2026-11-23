//! Fleet persistence in the shared host database.
//!
//! This feature owns these tables and applies [`MIGRATIONS`] under its feature
//! ID; builds without the feature never create them.

use anyhow::{Result, ensure};
use beale_storage::{
    HostDatabase,
    rusqlite::{self, OptionalExtension, params},
};
use std::collections::HashSet;

/// Ordered schema versions for this feature's tables.
pub const MIGRATIONS: &[&str] = &[V1];

const V1: &str = "
CREATE TABLE fleet_totp_secret (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    secret BLOB NOT NULL CHECK (length(secret) = 20)
);
CREATE TABLE manual_browsers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    executable TEXT NOT NULL UNIQUE
);
CREATE TABLE fleet_browser_settings (
    browser_id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    all_workspaces INTEGER NOT NULL CHECK (all_workspaces IN (0, 1))
);
CREATE TABLE fleet_browser_workspace_scope (
    browser_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY(browser_id, workspace_id),
    FOREIGN KEY(browser_id) REFERENCES fleet_browser_settings(browser_id) ON DELETE CASCADE
);
CREATE TABLE fleet_preferred_browser (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    browser_id TEXT NOT NULL
);
CREATE TABLE fleet_vm_settings (
    vm_id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    all_workspaces INTEGER NOT NULL CHECK (all_workspaces IN (0, 1)),
    can_run INTEGER NOT NULL CHECK (can_run IN (0, 1)),
    can_clone INTEGER NOT NULL CHECK (can_clone IN (0, 1))
);
CREATE TABLE fleet_vm_workspace_scope (
    vm_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY(vm_id, workspace_id),
    FOREIGN KEY(vm_id) REFERENCES fleet_vm_settings(vm_id) ON DELETE CASCADE
);
CREATE TABLE fleet_research_tool_settings (
    tool_id TEXT PRIMARY KEY,
    allow_locally INTEGER NOT NULL DEFAULT 0,
    allow_in_vms INTEGER NOT NULL DEFAULT 0
);
";

/// Creates or upgrades this feature's tables.
pub fn migrate(db: &HostDatabase) -> Result<()> {
    db.migrate_schema(crate::FEATURE_ID, MIGRATIONS)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualBrowserRow {
    pub id: String,
    pub name: String,
    pub executable: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserSettings {
    pub enabled: bool,
    pub all_workspaces: bool,
    pub workspace_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualMachineSettings {
    pub enabled: bool,
    pub all_workspaces: bool,
    pub workspace_ids: Vec<String>,
    pub can_run: bool,
    pub can_clone: bool,
}

impl Default for VirtualMachineSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            all_workspaces: true,
            workspace_ids: Vec::new(),
            can_run: false,
            can_clone: false,
        }
    }
}

/// Fleet settings on the host database.
pub trait FleetStore {
    fn manual_browsers(&self) -> Result<Vec<ManualBrowserRow>>;
    fn add_manual_browser(&self, row: &ManualBrowserRow) -> Result<()>;
    fn remove_manual_browser(&self, id: &str) -> Result<bool>;
    fn browser_settings(&self, id: &str) -> Result<BrowserSettings>;
    fn preferred_browser_id(&self) -> Result<Option<String>>;
    fn set_preferred_browser_id(&self, id: &str) -> Result<()>;
    fn set_browser_settings(&self, id: &str, settings: &BrowserSettings) -> Result<()>;
    fn research_tool_settings(&self, id: &str) -> Result<(bool, bool)>;
    fn set_research_tool_settings(
        &self,
        id: &str,
        allow_locally: bool,
        allow_in_vms: bool,
    ) -> Result<()>;
    fn virtual_machine_settings(&self, id: &str) -> Result<VirtualMachineSettings>;
    fn set_virtual_machine_settings(
        &self,
        id: &str,
        settings: &VirtualMachineSettings,
    ) -> Result<()>;
    fn fleet_totp_secret(&self) -> Result<Option<Vec<u8>>>;
    fn get_or_create_fleet_totp_secret(&self, candidate: &[u8]) -> Result<Vec<u8>>;
}

impl FleetStore for HostDatabase {
    fn manual_browsers(&self) -> Result<Vec<ManualBrowserRow>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id, name, executable FROM manual_browsers ORDER BY name, id")?;
            let rows = statement.query_map([], |row| {
                Ok(ManualBrowserRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    executable: row.get(2)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    fn add_manual_browser(&self, row: &ManualBrowserRow) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO manual_browsers(id, name, executable) VALUES (?1, ?2, ?3)",
                params![row.id, row.name, row.executable],
            )?;
            Ok(())
        })
    }

    fn remove_manual_browser(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let removed =
                transaction.execute("DELETE FROM manual_browsers WHERE id = ?1", [id])? > 0;
            if removed {
                transaction.execute(
                    "DELETE FROM fleet_browser_settings WHERE browser_id = ?1",
                    [id],
                )?;
                transaction.execute(
                    "DELETE FROM fleet_preferred_browser WHERE id = 1 AND browser_id = ?1",
                    [id],
                )?;
            }
            transaction.commit()?;
            Ok(removed)
        })
    }

    fn browser_settings(&self, id: &str) -> Result<BrowserSettings> {
        self.with_connection(|connection| {
            let (enabled, all_workspaces) = connection
                .query_row(
                    "SELECT enabled, all_workspaces FROM fleet_browser_settings WHERE browser_id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .unwrap_or((true, true));
            let mut statement = connection.prepare(
                "SELECT workspace_id FROM fleet_browser_workspace_scope WHERE browser_id = ?1 ORDER BY workspace_id",
            )?;
            let ids = statement.query_map([id], |row| row.get(0))?;
            Ok(BrowserSettings {
                enabled,
                all_workspaces,
                workspace_ids: ids.collect::<rusqlite::Result<Vec<_>>>()?,
            })
        })
    }

    fn preferred_browser_id(&self) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT browser_id FROM fleet_preferred_browser WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    fn set_preferred_browser_id(&self, id: &str) -> Result<()> {
        ensure!(
            !id.is_empty() && id.len() <= 80 && !id.chars().any(char::is_control),
            "invalid browser id"
        );
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO fleet_preferred_browser(id, browser_id) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET browser_id = excluded.browser_id",
                [id],
            )?;
            Ok(())
        })
    }

    fn set_browser_settings(&self, id: &str, settings: &BrowserSettings) -> Result<()> {
        ensure!(!id.is_empty() && id.len() <= 80, "invalid browser id");
        ensure!(
            settings.workspace_ids.len() <= 128,
            "too many selected workspaces"
        );
        ensure!(
            !settings.all_workspaces || settings.workspace_ids.is_empty(),
            "all-workspaces scope cannot include workspace ids"
        );
        let mut unique = HashSet::new();
        ensure!(
            settings.workspace_ids.iter().all(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && !id.chars().any(char::is_control)
                    && unique.insert(id)
            }),
            "invalid or duplicate workspace id"
        );
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO fleet_browser_settings(browser_id, enabled, all_workspaces) VALUES (?1, ?2, ?3)
                 ON CONFLICT(browser_id) DO UPDATE SET enabled = excluded.enabled, all_workspaces = excluded.all_workspaces",
                params![id, settings.enabled, settings.all_workspaces],
            )?;
            transaction.execute("DELETE FROM fleet_browser_workspace_scope WHERE browser_id = ?1", [id])?;
            for workspace_id in &settings.workspace_ids {
                transaction.execute(
                    "INSERT INTO fleet_browser_workspace_scope(browser_id, workspace_id) VALUES (?1, ?2)",
                    params![id, workspace_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    fn research_tool_settings(&self, id: &str) -> Result<(bool, bool)> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT allow_locally, allow_in_vms FROM fleet_research_tool_settings WHERE tool_id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .unwrap_or((false, false)))
        })
    }

    fn set_research_tool_settings(
        &self,
        id: &str,
        allow_locally: bool,
        allow_in_vms: bool,
    ) -> Result<()> {
        ensure!(
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
            "invalid research tool id"
        );
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO fleet_research_tool_settings(tool_id, allow_locally, allow_in_vms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(tool_id) DO UPDATE SET
                     allow_locally = excluded.allow_locally,
                     allow_in_vms = excluded.allow_in_vms",
                params![id, allow_locally, allow_in_vms],
            )?;
            Ok(())
        })
    }

    fn virtual_machine_settings(&self, id: &str) -> Result<VirtualMachineSettings> {
        self.with_connection(|connection| {
            let Some((enabled, all_workspaces, can_run, can_clone)) = connection
                .query_row(
                    "SELECT enabled, all_workspaces, can_run, can_clone FROM fleet_vm_settings WHERE vm_id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?
            else {
                return Ok(VirtualMachineSettings::default());
            };
            let mut statement = connection.prepare(
                "SELECT workspace_id FROM fleet_vm_workspace_scope WHERE vm_id = ?1 ORDER BY workspace_id",
            )?;
            let ids = statement.query_map([id], |row| row.get(0))?;
            Ok(VirtualMachineSettings {
                enabled,
                all_workspaces,
                workspace_ids: ids.collect::<rusqlite::Result<Vec<_>>>()?,
                can_run,
                can_clone,
            })
        })
    }

    fn set_virtual_machine_settings(
        &self,
        id: &str,
        settings: &VirtualMachineSettings,
    ) -> Result<()> {
        ensure!(
            !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
            "invalid VM id"
        );
        ensure!(
            settings.workspace_ids.len() <= 128,
            "too many selected workspaces"
        );
        ensure!(
            !settings.all_workspaces || settings.workspace_ids.is_empty(),
            "all-workspaces scope cannot include workspace ids"
        );
        let mut unique = HashSet::new();
        ensure!(
            settings.workspace_ids.iter().all(|workspace_id| {
                uuid::Uuid::parse_str(workspace_id).is_ok() && unique.insert(workspace_id)
            }),
            "invalid or duplicate workspace id"
        );
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO fleet_vm_settings(vm_id, enabled, all_workspaces, can_run, can_clone)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(vm_id) DO UPDATE SET enabled = excluded.enabled,
                     all_workspaces = excluded.all_workspaces,
                     can_run = excluded.can_run, can_clone = excluded.can_clone",
                params![
                    id,
                    settings.enabled,
                    settings.all_workspaces,
                    settings.can_run,
                    settings.can_clone
                ],
            )?;
            transaction.execute(
                "DELETE FROM fleet_vm_workspace_scope WHERE vm_id = ?1",
                [id],
            )?;
            for workspace_id in &settings.workspace_ids {
                transaction.execute(
                    "INSERT INTO fleet_vm_workspace_scope(vm_id, workspace_id) VALUES (?1, ?2)",
                    params![id, workspace_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    fn fleet_totp_secret(&self) -> Result<Option<Vec<u8>>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT secret FROM fleet_totp_secret WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    fn get_or_create_fleet_totp_secret(&self, candidate: &[u8]) -> Result<Vec<u8>> {
        ensure!(candidate.len() == 20, "invalid Fleet TOTP secret length");
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT OR IGNORE INTO fleet_totp_secret(id, secret) VALUES (1, ?1)",
                [candidate],
            )?;
            let secret: Vec<u8> = transaction.query_row(
                "SELECT secret FROM fleet_totp_secret WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            transaction.commit()?;
            ensure!(secret.len() == 20, "invalid stored Fleet TOTP secret");
            Ok(secret)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "beale-feature-store-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
    }

    fn open(root: &std::path::Path) -> HostDatabase {
        let db = HostDatabase::open(root).unwrap();
        migrate(&db).unwrap();
        db
    }

    #[test]
    fn vm_allowances_default_off_and_persist_changes() {
        let root = root();
        let migrated = open(&root);
        let id = "hyper-v:00000000-0000-4000-8000-000000000001";
        assert_eq!(
            migrated.virtual_machine_settings(id).unwrap(),
            VirtualMachineSettings::default()
        );
        let selected = VirtualMachineSettings {
            enabled: false,
            all_workspaces: false,
            workspace_ids: vec![uuid::Uuid::nil().to_string()],
            can_run: true,
            can_clone: true,
        };
        migrated
            .set_virtual_machine_settings(id, &selected)
            .unwrap();
        drop(migrated);
        let reopened = open(&root);
        assert_eq!(reopened.virtual_machine_settings(id).unwrap(), selected);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn research_tool_approvals_default_off_and_survive_reopen() {
        let root = root();
        let db = open(&root);
        assert_eq!(
            db.research_tool_settings("ripgrep").unwrap(),
            (false, false)
        );
        db.set_research_tool_settings("ripgrep", true, false)
            .unwrap();
        db.set_research_tool_settings("ghidra", false, true)
            .unwrap();
        drop(db);
        let reopened = open(&root);
        assert_eq!(
            reopened.research_tool_settings("ripgrep").unwrap(),
            (true, false)
        );
        assert_eq!(
            reopened.research_tool_settings("ghidra").unwrap(),
            (false, true)
        );
        assert_eq!(
            reopened.research_tool_settings("jadx").unwrap(),
            (false, false)
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fleet_totp_secret_is_created_once_and_survives_restart() {
        let root = root();
        let db = open(&root);
        assert!(db.fleet_totp_secret().unwrap().is_none());
        assert_eq!(
            db.get_or_create_fleet_totp_secret(&[7; 20]).unwrap(),
            [7; 20]
        );
        assert_eq!(
            db.get_or_create_fleet_totp_secret(&[8; 20]).unwrap(),
            [7; 20]
        );
        drop(db);
        let reopened = open(&root);
        assert_eq!(reopened.fleet_totp_secret().unwrap().unwrap(), [7; 20]);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
