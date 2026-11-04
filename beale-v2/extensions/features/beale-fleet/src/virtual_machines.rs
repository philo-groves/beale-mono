//! Read-only VM discovery on the app-server host.

use beale_contracts::v1::FleetVirtualMachine;
#[cfg(any(windows, test))]
use serde::Deserialize;

#[cfg(any(windows, test))]
const MAX_VMS: usize = 256;

#[cfg(any(windows, test))]
#[derive(Deserialize)]
struct HyperVRow {
    id: String,
    name: String,
    state: String,
    generation: u32,
}

#[cfg(any(windows, test))]
fn parse_hyper_v(bytes: &[u8]) -> Result<Vec<FleetVirtualMachine>, ()> {
    let rows: Vec<HyperVRow> = serde_json::from_slice(bytes).map_err(|_| ())?;
    if rows.len() > MAX_VMS {
        return Err(());
    }
    let mut machines = Vec::with_capacity(rows.len());
    for row in rows {
        let id = uuid::Uuid::parse_str(&row.id).map_err(|_| ())?;
        if row.name.trim().is_empty()
            || row.name.len() > 120
            || row.name.chars().any(char::is_control)
            || !matches!(row.generation, 1 | 2)
        {
            return Err(());
        }
        let state = match row.state.as_str() {
            "Running" => "Running",
            "Off" => "Off",
            "Saved" => "Saved",
            "Paused" => "Paused",
            "Starting" => "Starting",
            "Stopping" => "Stopping",
            "Pausing" => "Pausing",
            "Resuming" => "Resuming",
            "Saving" => "Saving",
            "FastSaved" => "Fast Saved",
            "FastSaving" => "Fast Saving",
            "RunningCritical" => "Running (Critical)",
            "OffCritical" => "Off (Critical)",
            "PausedCritical" => "Paused (Critical)",
            "SavedCritical" => "Saved (Critical)",
            _ => "Unknown",
        };
        machines.push(FleetVirtualMachine {
            id: format!("hyper-v:{id}"),
            name: row.name,
            provider: "Hyper-V".to_owned(),
            state: state.to_owned(),
            generation: row.generation,
            enabled: true,
            all_workspaces: true,
            workspace_ids: Vec::new(),
            can_run: false,
            can_clone: false,
        });
    }
    machines.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(machines)
}

#[cfg(windows)]
pub(crate) async fn inventory() -> Result<Vec<FleetVirtualMachine>, ()> {
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::{process::Command, time::timeout};

    let system_root = std::env::var_os("SystemRoot").ok_or(())?;
    let powershell =
        PathBuf::from(system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if !powershell.is_file() {
        return Err(());
    }
    // This fixed script reads only the local Hyper-V inventory. VM names never enter commands.
    const SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); if (-not (Get-Command Get-VM -ErrorAction SilentlyContinue) -or -not (Get-Service vmms -ErrorAction SilentlyContinue)) { '[]'; exit 0 }; $vms = @(Get-VM -ErrorAction Stop | Select-Object -First 257); if ($vms.Count -gt 256) { exit 2 }; $rows = @($vms | ForEach-Object { [pscustomobject]@{ id = $_.VMId.ToString(); name = $_.Name; state = $_.State.ToString(); generation = [int]$_.Generation } }); ConvertTo-Json -InputObject $rows -Compress -Depth 3"#;
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        .kill_on_drop(true);
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = timeout(Duration::from_secs(12), command.output())
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    if !output.status.success() || output.stdout.len() > 131_072 {
        return Err(());
    }
    parse_hyper_v(&output.stdout)
}

#[cfg(not(windows))]
pub(crate) async fn inventory() -> Result<Vec<FleetVirtualMachine>, ()> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_hyper_v_inventory_without_host_paths() {
        let machines = parse_hyper_v(
            br#"[{"id":"00000000-0000-4000-8000-000000000001","name":"Example VM","state":"Running","generation":2}]"#,
        )
        .unwrap();
        assert_eq!(machines.len(), 1);
        assert_eq!(
            machines[0].id,
            "hyper-v:00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(machines[0].provider, "Hyper-V");
        assert_eq!(machines[0].state, "Running");
        assert_eq!(machines[0].generation, 2);
        assert!(
            parse_hyper_v(
                br#"[{"id":"invalid","name":"Example VM","state":"Off","generation":1}]"#
            )
            .is_err()
        );
        assert!(parse_hyper_v(br#"[{"id":"00000000-0000-4000-8000-000000000001","name":"Bad\nName","state":"Off","generation":1}]"#).is_err());
    }
}
