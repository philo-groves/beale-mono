//! Resource capacity and availability on the app-server host.

use beale_contracts::v1::FleetSystemResources;
use std::path::Path;
use sysinfo::{RefreshKind, System, SystemExt};

pub(super) fn snapshot(data_root: &Path) -> FleetSystemResources {
    let cpu_logical_cores = std::thread::available_parallelism()
        .ok()
        .and_then(|count| u32::try_from(count.get()).ok())
        .unwrap_or(0);
    let system = System::new_with_specifics(RefreshKind::new().with_memory());
    let root = data_root
        .canonicalize()
        .unwrap_or_else(|_| data_root.to_path_buf());
    let (disk_total_bytes, disk_available_bytes) = disk_space(&root).unwrap_or((0, 0));
    let memory_total_bytes = system.total_memory();
    FleetSystemResources {
        cpu_logical_cores,
        memory_total_bytes,
        memory_available_bytes: system.available_memory().min(memory_total_bytes),
        disk_total_bytes,
        disk_available_bytes,
    }
}

#[cfg(windows)]
fn disk_space(data_root: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let path: Vec<u16> = data_root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut available = 0;
    let mut total = 0;
    // SAFETY: `path` is NUL-terminated and remains alive for the call; the output pointers
    // reference writable u64 values. The final optional output is not needed.
    let success = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            &mut total,
            std::ptr::null_mut(),
        )
    };
    (success != 0 && total > 0).then_some((total, available.min(total)))
}

#[cfg(unix)]
fn disk_space(data_root: &Path) -> Option<(u64, u64)> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    fn to_u64(value: impl TryInto<u64>) -> Option<u64> {
        value.try_into().ok()
    }

    let path = CString::new(data_root.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and valid for this call; `stats` is writable and
    // initialized by a successful `statvfs` before it is read.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: `statvfs` returned success and initialized the output structure.
    let stats = unsafe { stats.assume_init() };
    let block_size = to_u64(stats.f_frsize)?;
    let total = to_u64(stats.f_blocks)?.saturating_mul(block_size);
    let available = to_u64(stats.f_bavail)?.saturating_mul(block_size);
    (total > 0).then_some((total, available.min(total)))
}

#[cfg(not(any(windows, unix)))]
fn disk_space(_data_root: &Path) -> Option<(u64, u64)> {
    None
}
