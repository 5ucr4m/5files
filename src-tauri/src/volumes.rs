//! Local volumes (drives) and well-known places (Home, Desktop…), per platform.

use serde::Serialize;
use sysinfo::{DiskKind, Disks};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub mount: String,
    pub total: u64,
    pub free: u64,
    pub kind: &'static str,
    pub removable: bool,
    pub fs_type: String,
    pub is_root: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: &'static str,
    pub name: String,
    pub path: String,
}

const PSEUDO_FS: &[&str] = &[
    "tmpfs", "devtmpfs", "overlay", "squashfs", "proc", "sysfs", "efivarfs", "devfs", "autofs", "ramfs", "cgroup",
    "cgroup2", "fuse.portal", "nullfs", "bpf", "tracefs", "debugfs", "securityfs", "pstore", "configfs",
];

/// Decides whether a mount point is something a user would consider "a drive".
pub fn is_user_visible(os: &str, mount: &str, fs_type: &str) -> bool {
    if PSEUDO_FS.contains(&fs_type.to_ascii_lowercase().as_str()) {
        return false;
    }
    match os {
        "macos" => mount == "/" || (mount.starts_with("/Volumes/") && !mount.starts_with("/Volumes/Recovery")),
        "linux" => {
            mount == "/"
                || mount == "/home"
                || mount.starts_with("/media/")
                || mount.starts_with("/mnt/")
                || mount.starts_with("/run/media/")
        }
        _ => true,
    }
}

pub fn list() -> Vec<Volume> {
    let disks = Disks::new_with_refreshed_list();
    let os = std::env::consts::OS;
    let mut out: Vec<Volume> = disks
        .list()
        .iter()
        .filter_map(|d| {
            let mount = d.mount_point().to_string_lossy().into_owned();
            let fs_type = d.file_system().to_string_lossy().into_owned();
            if !is_user_visible(os, &mount, &fs_type) {
                return None;
            }
            let is_root = mount == "/" || (cfg!(windows) && mount.eq_ignore_ascii_case(&windows_system_drive()));
            let raw_name = d.name().to_string_lossy().into_owned();
            let name = if !raw_name.is_empty() && !raw_name.starts_with("/dev/") {
                if cfg!(windows) { format!("{raw_name} ({})", mount.trim_end_matches('\\')) } else { raw_name }
            } else if mount == "/" {
                "System".into()
            } else {
                std::path::Path::new(&mount)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| mount.clone())
            };
            Some(Volume {
                name,
                mount,
                total: d.total_space(),
                free: d.available_space(),
                kind: match d.kind() {
                    DiskKind::SSD => "ssd",
                    DiskKind::HDD => "hdd",
                    _ => "unknown",
                },
                removable: d.is_removable(),
                fs_type,
                is_root,
            })
        })
        .collect();
    // Same device can be reported twice (e.g. btrfs subvolumes): dedupe by mount.
    out.sort_by(|a, b| b.is_root.cmp(&a.is_root).then(a.mount.cmp(&b.mount)));
    out.dedup_by(|a, b| a.mount == b.mount);
    out
}

fn windows_system_drive() -> String {
    std::env::var("SystemDrive").map(|d| format!("{d}\\")).unwrap_or_else(|_| "C:\\".into())
}

pub fn places() -> Vec<Place> {
    let candidates: [(&'static str, &str, Option<std::path::PathBuf>); 7] = [
        ("home", "Home", dirs::home_dir()),
        ("desktop", "Desktop", dirs::desktop_dir()),
        ("documents", "Documents", dirs::document_dir()),
        ("downloads", "Downloads", dirs::download_dir()),
        ("pictures", "Pictures", dirs::picture_dir()),
        ("music", "Music", dirs::audio_dir()),
        ("videos", "Videos", dirs::video_dir()),
    ];
    candidates
        .into_iter()
        .filter_map(|(id, name, p)| {
            let p = p?;
            p.is_dir().then(|| Place { id, name: name.into(), path: p.to_string_lossy().into_owned() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_pseudo_and_system_mounts() {
        assert!(is_user_visible("macos", "/", "apfs"));
        assert!(is_user_visible("macos", "/Volumes/USB", "msdos"));
        assert!(!is_user_visible("macos", "/System/Volumes/Data", "apfs"));
        assert!(is_user_visible("linux", "/media/ana/USB", "vfat"));
        assert!(!is_user_visible("linux", "/snap/core/1", "squashfs"));
        assert!(!is_user_visible("linux", "/run/user/1000", "tmpfs"));
        assert!(!is_user_visible("linux", "/boot/efi", "vfat"));
        assert!(is_user_visible("windows", "D:\\", "NTFS"));
    }

    #[test]
    fn lists_at_least_the_root_volume_and_home() {
        let v = list();
        assert!(!v.is_empty(), "expected at least one volume");
        assert!(v.iter().any(|v| v.total > 0));
        assert!(places().iter().any(|p| p.id == "home"));
    }
}
