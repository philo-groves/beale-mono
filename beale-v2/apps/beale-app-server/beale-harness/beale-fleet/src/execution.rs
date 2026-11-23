//! Operator-configured OpenSSH execution in discovered Windows Hyper-V guests.

#[cfg(any(windows, test))]
use anyhow::ensure;
use anyhow::{Result, bail};
use beale_contracts::v1::FleetVirtualMachine;
use beale_storage::HostDatabase;
use beale_tools::CommandOutput;

use crate::{FleetStore, VirtualMachineSettings, virtual_machines};

// Windows OpenSSH may launch the remote command through cmd.exe (8191 chars).
#[cfg(windows)]
const MAX_ENCODED_COMMAND_CHARS: usize = 7_000;

#[cfg(any(windows, test))]
fn ssh_alias(machine: &str) -> Result<String> {
    let id = machine
        .strip_prefix("hyper-v:")
        .ok_or_else(|| anyhow::anyhow!("invalid Fleet machine ID"))?;
    let uuid =
        uuid::Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("invalid Fleet machine ID"))?;
    ensure!(
        machine == format!("hyper-v:{uuid}"),
        "invalid Fleet machine ID"
    );
    Ok(format!("beale-hyper-v-{uuid}"))
}

#[cfg(any(windows, test))]
fn encoded_powershell_command(command: &str) -> String {
    use base64::Engine;
    let script = beale_tools::powershell_script(command);
    let utf16 = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

fn permitted(
    machine: &FleetVirtualMachine,
    settings: &VirtualMachineSettings,
    workspace_id: &str,
) -> bool {
    machine.provider == "Hyper-V"
        && machine.state == "Running"
        && settings.enabled
        && settings.can_run
        && (settings.all_workspaces || settings.workspace_ids.iter().any(|id| id == workspace_id))
}

/// Lists only running VMs approved for this workspace, without SSH details.
pub async fn machine_prompt_note(db: &HostDatabase, workspace_id: &str) -> Result<Option<String>> {
    let Ok(inventory) = virtual_machines::inventory().await else {
        return Ok(None);
    };
    let mut lines = Vec::new();
    for machine in inventory {
        let settings = db.virtual_machine_settings(&machine.id)?;
        if permitted(&machine, &settings, workspace_id) {
            lines.push(format!(
                "- {} (machine: `{}`)",
                serde_json::to_string(&machine.name)?,
                machine.id
            ));
        }
    }
    if lines.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "Approved running Hyper-V guests for this workspace (Windows PowerShell over operator-configured OpenSSH):\n{}\nSet the optional `machine` argument of `powershell` to an ID above. Omit it for host execution. Guest commands start in the SSH user's directory; host workspace files are not automatically present there.",
        lines.join("\n")
    )))
}

/// Rechecks discovery, state, Can Run, and workspace scope on every call.
#[cfg(windows)]
pub async fn run_machine_command(
    db: &HostDatabase,
    workspace_id: &str,
    machine_id: &str,
    command: &str,
    workspace: &std::path::Path,
    timeout_seconds: u64,
) -> Result<CommandOutput> {
    use beale_tools::process::contained_command;
    use std::path::PathBuf;

    let alias = ssh_alias(machine_id)?;
    ensure!(
        !command.trim().is_empty() && command.len() <= beale_tools::MAX_COMMAND_BYTES,
        "invalid command"
    );
    ensure!(
        (1..=beale_tools::MAX_COMMAND_TIMEOUT_SECONDS).contains(&timeout_seconds),
        "invalid command timeout_seconds"
    );
    let inventory = virtual_machines::inventory()
        .await
        .map_err(|_| anyhow::anyhow!("Fleet VM inventory unavailable"))?;
    let machine = inventory
        .iter()
        .find(|machine| machine.id == machine_id)
        .ok_or_else(|| anyhow::anyhow!("Fleet machine is not available"))?;
    let settings = db
        .virtual_machine_settings(machine_id)
        .map_err(|_| anyhow::anyhow!("Fleet VM settings unavailable"))?;
    ensure!(
        permitted(machine, &settings, workspace_id),
        "Fleet machine is not running or allowed in this workspace"
    );

    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| anyhow::anyhow!("Windows system root unavailable"))?;
    let ssh = PathBuf::from(system_root).join("System32/OpenSSH/ssh.exe");
    ensure!(ssh.is_file(), "Windows OpenSSH client unavailable");
    let encoded = encoded_powershell_command(command);
    ensure!(
        encoded.len() <= MAX_ENCODED_COMMAND_CHARS,
        "Fleet guest command is too long"
    );
    let mut process = contained_command(ssh);
    process.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "PasswordAuthentication=no",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "PreferredAuthentications=publickey",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "PermitLocalCommand=no",
        &alias,
        &format!("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {encoded}"),
    ]);
    let result = beale_tools::run_contained_process(process, workspace, timeout_seconds).await?;
    // SSH transport errors can include private host configuration paths or endpoints.
    if result.exit_code == Some(255) {
        bail!("Fleet machine SSH connection unavailable");
    }
    Ok(result)
}

#[cfg(not(windows))]
pub async fn run_machine_command(
    _db: &HostDatabase,
    _workspace_id: &str,
    _machine_id: &str,
    _command: &str,
    _workspace: &std::path::Path,
    _timeout_seconds: u64,
) -> Result<CommandOutput> {
    bail!("Hyper-V guest execution is available only on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_is_derived_only_from_canonical_hyper_v_ids() {
        assert_eq!(
            ssh_alias("hyper-v:00000000-0000-4000-8000-000000000001").unwrap(),
            "beale-hyper-v-00000000-0000-4000-8000-000000000001"
        );
        for id in [
            "local",
            "hyper-v:bad",
            "hyper-v:{00000000-0000-4000-8000-000000000001}",
        ] {
            assert!(ssh_alias(id).is_err());
        }
    }

    #[test]
    fn permission_requires_running_state_run_allowance_and_workspace_scope() {
        let machine = FleetVirtualMachine {
            id: "hyper-v:00000000-0000-4000-8000-000000000001".to_owned(),
            name: "Example VM".to_owned(),
            provider: "Hyper-V".to_owned(),
            state: "Running".to_owned(),
            generation: 2,
            enabled: true,
            all_workspaces: true,
            workspace_ids: Vec::new(),
            can_run: false,
        };
        let workspace_id = uuid::Uuid::nil().to_string();
        let mut settings = VirtualMachineSettings::default();
        assert!(!permitted(&machine, &settings, &workspace_id));
        settings.can_run = true;
        assert!(permitted(&machine, &settings, &workspace_id));
        settings.all_workspaces = false;
        assert!(!permitted(&machine, &settings, &workspace_id));
        settings.workspace_ids.push(workspace_id.clone());
        assert!(permitted(&machine, &settings, &workspace_id));
        settings.enabled = false;
        assert!(!permitted(&machine, &settings, &workspace_id));
        settings.enabled = true;
        let mut off = machine.clone();
        off.state = "Off".to_owned();
        assert!(!permitted(&off, &settings, &workspace_id));
    }

    #[test]
    fn remote_command_is_encoded_as_utf16_powershell_without_shell_metacharacters() {
        use base64::Engine;
        let encoded = encoded_powershell_command("Write-Output 'example; Ω'");
        assert!(
            encoded
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || matches!(character, '+' | '/' | '='))
        );
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let units = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let script = String::from_utf16(&units).unwrap();
        assert!(script.contains("Write-Output 'example; Ω'"));
        assert!(script.contains("$ErrorActionPreference = 'Stop'"));
    }
}
