use super::*;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::Path,
    time::Duration,
};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Info {
    mem_total: u64,
    docker_root_dir: String,
    operating_system: String,
    #[serde(rename = "OSType")]
    os_type: String,
}
#[derive(Deserialize)]
struct Stats {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "MemUsage")]
    memory: String,
}
async fn docker_json(env: &DockerEnvironments, args: &[&str]) -> Result<Vec<u8>> {
    env.docker
        .run(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            None,
            Duration::from_secs(15),
        )
        .await
}

pub async fn measure(
    root: &Path,
    env: &DockerEnvironments,
    allocations: &[Allocation],
) -> Result<Capacity> {
    let info: Info =
        serde_json::from_slice(&docker_json(env, &["info", "--format", "{{json .}}"]).await?)?;
    ensure!(
        info.os_type == "linux" && info.mem_total > 0,
        "Docker execution-domain memory is unknown"
    );
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        ensure!(
            host.starts_with("unix://") || host.starts_with("npipe://"),
            "Measure capacity on the Docker daemon's owning Fleet host; remote daemon capacity is unknown"
        );
    }
    let endpoint: String = serde_json::from_slice(
        &docker_json(
            env,
            &[
                "context",
                "inspect",
                "--format",
                "{{json .Endpoints.docker.Host}}",
            ],
        )
        .await?,
    )?;
    ensure!(
        endpoint.starts_with("unix://") || endpoint.starts_with("npipe://"),
        "Remote Docker context capacity is unknown; use the daemon's owning Fleet host"
    );
    let raw = docker_json(
        env,
        &[
            "stats",
            "--all",
            "--no-stream",
            "--no-trunc",
            "--format",
            "{{json .}}",
        ],
    )
    .await?;
    let mut usage = BTreeMap::new();
    for line in std::str::from_utf8(&raw)?
        .lines()
        .filter(|s| !s.trim().is_empty())
    {
        let row: Stats = serde_json::from_str(line)?;
        usage.insert(
            row.id,
            parse_bytes(
                row.memory
                    .split('/')
                    .next()
                    .context("Docker memory measurement missing")?,
            )?,
        );
    }
    let mut measured_memory = BTreeMap::new();
    let mut measured_storage = BTreeSet::new();
    for allocation in allocations {
        if let Some(instance) = env
            .store
            .instance_if_present(allocation.request.instance.parse()?)?
        {
            if let Some(handle) = &instance.handle
                && let Some(bytes) = usage.get(&handle.id)
            {
                measured_memory.insert(allocation.request.id.clone(), *bytes);
            }
            if instance.prepared {
                measured_storage.extend(
                    allocation
                        .request
                        .storage
                        .iter()
                        .filter(|d| {
                            d.key.starts_with("image-")
                                || instance.target.is_some()
                                    && instance.stopped
                                    && allocation.phase == AllocationPhase::Retained
                        })
                        .map(|d| d.key.clone()),
                );
            }
        }
    }
    let (memory, docker_pool, docker_free) = domain_resources(&info).await?;
    let (host_pool, host_free) = host_storage(root)?;
    let mut storage = BTreeMap::from([(docker_pool, docker_free)]);
    storage
        .entry(host_pool)
        .and_modify(|free| *free = (*free).min(host_free))
        .or_insert(host_free);
    #[cfg(windows)]
    for path in desktop_backing_paths()? {
        let (pool, available) = host_storage(&path)?;
        storage
            .entry(pool)
            .and_modify(|n| *n = (*n).min(available))
            .or_insert(available);
    }
    let allocated = usage.values().fold(0u64, |n, v| n.saturating_add(*v));
    Ok(Capacity {
        domain: "docker".into(),
        observed_at_ms: Allocator::now(),
        available_memory: memory.min(info.mem_total.saturating_sub(allocated)),
        storage,
        measured_memory,
        measured_storage,
    })
}
fn parse_bytes(value: &str) -> Result<u64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .context("Docker memory unit missing")?;
    let number: f64 = value[..split].parse()?;
    let unit = value[split..].trim();
    let multiplier = match unit {
        "B" => 1.0,
        "kB" => 1000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "KiB" => 1024.0,
        "MiB" => 1_048_576.0,
        "GiB" => 1_073_741_824.0,
        "TiB" => 1_099_511_627_776.0,
        _ => anyhow::bail!("unknown Docker memory unit"),
    };
    let bytes = number * multiplier;
    ensure!(
        bytes.is_finite() && bytes >= 0.0 && bytes < u64::MAX as f64,
        "invalid Docker memory measurement"
    );
    Ok(bytes.floor() as u64)
}
fn mem_available(text: &str) -> Result<u64> {
    let line = text
        .lines()
        .find(|l| l.starts_with("MemAvailable:"))
        .context("Execution-domain available memory is unknown")?;
    let mut parts = line.split_whitespace();
    parts.next();
    let kb: u64 = parts.next().context("memory value missing")?.parse()?;
    ensure!(parts.next() == Some("kB"), "invalid memory unit");
    kb.checked_mul(1024).context("memory overflow")
}

#[cfg(target_os = "linux")]
async fn domain_resources(info: &Info) -> Result<(u64, String, u64)> {
    ensure!(
        !info.operating_system.contains("Docker Desktop"),
        "Docker Desktop VM capacity must be measured by its owning desktop host"
    );
    let memory = local_memory()?;
    let (pool, available) = host_storage(Path::new(&info.docker_root_dir))
        .context("Docker backing storage is not accessible on this owning host")?;
    Ok((memory, pool, available))
}
#[cfg(windows)]
async fn domain_resources(info: &Info) -> Result<(u64, String, u64)> {
    ensure!(
        info.operating_system.contains("Docker Desktop"),
        "Remote Docker execution-domain capacity is unknown; use its owning Fleet host"
    );
    // Docker Desktop's Linux VM owns the actual RAM and ext4 capacity. Host
    // Windows free RAM alone cannot admit Linux guest research.
    let run = async |args: &[&str]| -> Result<String> {
        let out = tokio::time::timeout(
            Duration::from_secs(15),
            beale_tools::process::contained_command("wsl.exe")
                .args(["--distribution", "docker-desktop", "--exec"])
                .args(args)
                .output(),
        )
        .await??;
        ensure!(
            out.status.success() && out.stdout.len() <= 65536,
            "Docker Desktop capacity unavailable; check its WSL execution domain"
        );
        Ok(String::from_utf8(out.stdout)?)
    };
    let memory = mem_available(&run(&["cat", "/proc/meminfo"]).await?)?;
    let pid = run(&["pidof", "dockerd"]).await?;
    let pid = pid.trim();
    ensure!(
        !pid.is_empty() && pid.bytes().all(|c| c.is_ascii_digit()),
        "Docker daemon execution namespace is unknown"
    );
    ensure!(
        info.docker_root_dir.starts_with('/') && !info.docker_root_dir.contains(".."),
        "invalid daemon storage path"
    );
    let daemon_path = format!("/proc/{pid}/root{}", info.docker_root_dir);
    let data = run(&["stat", "-f", "-c", "%i %a %S", &daemon_path]).await?;
    let fields: Vec<_> = data.split_whitespace().collect();
    ensure!(
        fields.len() == 3 && fields[0].bytes().all(|b| b.is_ascii_hexdigit()),
        "Docker filesystem identity unavailable"
    );
    let available = fields[1]
        .parse::<u64>()?
        .checked_mul(fields[2].parse::<u64>()?)
        .context("Docker free-space overflow")?;
    Ok((memory, format!("docker-fs-{}", fields[0]), available))
}

/// The execution process's own limits matter even for native local workers.
pub fn local_memory() -> Result<u64> {
    use sysinfo::{System, SystemExt};
    let mut system = System::new();
    system.refresh_memory();
    let available = system.available_memory();
    #[cfg(target_os = "linux")]
    {
        let mut available =
            available.min(mem_available(&std::fs::read_to_string("/proc/meminfo")?)?);
        let cgroup = std::fs::read_to_string("/proc/self/cgroup")?;
        if let Some(path) = cgroup.lines().find_map(|line| line.strip_prefix("0::")) {
            ensure!(
                path.starts_with('/') && !path.split('/').any(|p| p == ".."),
                "invalid execution cgroup"
            );
            let base = Path::new("/sys/fs/cgroup");
            let mut directory = base.join(path.trim_start_matches('/'));
            loop {
                let max = directory.join("memory.max");
                if max.try_exists()? {
                    let max = std::fs::read_to_string(max)?;
                    if max.trim() != "max" {
                        let current: u64 =
                            std::fs::read_to_string(directory.join("memory.current"))?
                                .trim()
                                .parse()?;
                        available =
                            available.min(max.trim().parse::<u64>()?.saturating_sub(current));
                    }
                }
                if directory == base {
                    break;
                }
                ensure!(
                    directory.pop() && directory.starts_with(base),
                    "execution cgroup escaped root"
                );
            }
        }
        ensure!(available > 0, "execution-domain memory unavailable");
        Ok(available)
    }
    #[cfg(not(target_os = "linux"))]
    {
        ensure!(available > 0, "available memory unknown");
        Ok(available)
    }
}

#[cfg(windows)]
fn desktop_backing_paths() -> Result<Vec<PathBuf>> {
    // Operator override supports relocated/managed Desktop disks. Otherwise
    // discover actual VHDX files, not merely a configured Hyper-V directory.
    let mut roots = Vec::new();
    if let Some(path) = std::env::var_os("BEALE_DOCKER_BACKING_STORAGE") {
        roots.push(PathBuf::from(path));
    } else {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Docker/wsl"));
        }
        if let Some(roaming) = std::env::var_os("APPDATA") {
            let path = PathBuf::from(roaming).join("Docker/settings-store.json");
            if path.try_exists()? {
                let settings: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
                for key in [
                    "diskImageLocation",
                    "DiskImageLocation",
                    "dataFolder",
                    "DataFolder",
                ] {
                    if let Some(value) = settings
                        .get(key)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        roots.push(PathBuf::from(value));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    let mut queue = roots.into_iter().map(|p| (p, 0)).collect::<Vec<_>>();
    while let Some((path, depth)) = queue.pop() {
        if !path.try_exists()? {
            continue;
        }
        if path.is_file() {
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("vhdx"))
            {
                found.push(path);
            }
            continue;
        }
        if depth >= 3 {
            continue;
        }
        for entry in std::fs::read_dir(path)?.take(128) {
            let entry = entry?;
            if !entry.file_type()?.is_symlink() {
                queue.push((entry.path(), depth + 1));
            }
        }
    }
    ensure!(
        !found.is_empty(),
        "Docker backing volume is unknown; set BEALE_DOCKER_BACKING_STORAGE to the actual Docker data VHDX or its directory"
    );
    Ok(found)
}
#[cfg(not(any(windows, target_os = "linux")))]
async fn domain_resources(_info: &Info) -> Result<(u64, String, u64)> {
    anyhow::bail!(
        "Docker execution-domain capacity unavailable; connect a Fleet host that exposes its Linux memory and backing storage"
    )
}

pub fn host_storage(path: &Path) -> Result<(String, u64)> {
    let path = path.canonicalize()?;
    let path = if path.is_file() {
        path.parent()
            .context("storage file has no parent")?
            .to_path_buf()
    } else {
        path
    };
    let (_, available) =
        crate::resources::disk_space(&path).context("Storage capacity unavailable")?;
    #[cfg(unix)]
    let id = {
        use std::os::unix::fs::MetadataExt;
        format!("filesystem-{}", std::fs::metadata(&path)?.dev())
    };
    #[cfg(windows)]
    let id = {
        use sysinfo::{DiskExt, System, SystemExt};
        let mut system = System::new();
        system.refresh_disks_list();
        let normalized = path
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase();
        let disk = system
            .disks()
            .iter()
            .filter(|disk| {
                normalized.starts_with(&disk.mount_point().to_string_lossy().to_lowercase())
            })
            .max_by_key(|disk| disk.mount_point().components().count())
            .context("Storage volume identity unavailable")?;
        use sha2::{Digest, Sha256};
        format!(
            "filesystem-{:x}",
            Sha256::digest(
                disk.mount_point()
                    .to_string_lossy()
                    .to_lowercase()
                    .as_bytes()
            )
        )
    };
    #[cfg(not(any(unix, windows)))]
    let id = return Err(anyhow::anyhow!("storage measurement unsupported"));
    Ok((id, available))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_actual_domain_measurements() {
        assert_eq!(
            mem_available("MemTotal: 90000000 kB\nMemAvailable: 1234 kB\n").unwrap(),
            1234 * 1024
        );
        assert!(mem_available("MemTotal: 90000000 kB").is_err());
        assert_eq!(parse_bytes("1.5GiB").unwrap(), 1_610_612_736);
        assert!(parse_bytes("unknown").is_err());
    }
}
