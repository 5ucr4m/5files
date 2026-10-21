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
    /// Can be ejected / safely removed from the sidebar.
    pub ejectable: bool,
    /// OS device (e.g. /dev/sdb1) — needed to eject on Linux.
    #[serde(skip)]
    pub device: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: &'static str,
    pub name: String,
    pub path: String,
}

const PSEUDO_FS: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "overlay",
    "squashfs",
    "proc",
    "sysfs",
    "efivarfs",
    "devfs",
    "autofs",
    "ramfs",
    "cgroup",
    "cgroup2",
    "fuse.portal",
    "nullfs",
    "bpf",
    "tracefs",
    "debugfs",
    "securityfs",
    "pstore",
    "configfs",
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
                if cfg!(windows) { format!("{raw_name} ({})", mount.trim_end_matches('\\')) } else { raw_name.clone() }
            } else if mount == "/" {
                "System".into()
            } else {
                std::path::Path::new(&mount).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| mount.clone())
            };
            let removable = d.is_removable();
            let ejectable = !is_root && is_ejectable(os, &mount, removable);
            Some(Volume {
                ejectable,
                device: raw_name.clone(),
                name,
                mount,
                total: d.total_space(),
                free: d.available_space(),
                kind: match d.kind() {
                    DiskKind::SSD => "ssd",
                    DiskKind::HDD => "hdd",
                    _ => "unknown",
                },
                removable,
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

/// External disks, SD cards and mounted disk images — but never the system volume.
pub fn is_ejectable(os: &str, mount: &str, removable: bool) -> bool {
    match os {
        // Everything under /Volumes is an attached disk or image (the system disk is "/").
        "macos" => mount.starts_with("/Volumes/"),
        "linux" => removable || mount.starts_with("/media/") || mount.starts_with("/run/media/"),
        _ => removable,
    }
}

/// Ejects (unmounts and, where possible, powers off) the volume mounted at `mount`.
pub async fn eject(mount: &str, device: &str) -> Result<(), String> {
    use tokio::process::Command;
    async fn run(cmd: &mut Command) -> Result<(), String> {
        let out = cmd.output().await.map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let msg = if msg.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { msg };
            Err(if msg.is_empty() { format!("exit status {}", out.status) } else { msg })
        }
    }
    let _ = device;
    #[cfg(target_os = "macos")]
    {
        run(Command::new("diskutil").arg("eject").arg(mount)).await
    }
    #[cfg(target_os = "linux")]
    {
        // Desktop sessions: gio does the right thing (unmount + power off).
        if run(Command::new("gio").args(["mount", "--eject"]).arg(mount)).await.is_ok() {
            return Ok(());
        }
        run(Command::new("udisksctl").args(["unmount", "-b", device])).await?;
        // Power-off is best effort (not every device supports it).
        let parent = device.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('p');
        let _ = run(Command::new("udisksctl").args(["power-off", "-b", parent])).await;
        Ok(())
    }
    #[cfg(windows)]
    {
        // The same "Eject" Explorer offers; it reports nothing, so verify afterwards.
        let drive = mount.trim_end_matches('\\');
        let script = format!("(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{drive}').InvokeVerb('Eject')");
        run(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script])).await?;
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if !std::path::Path::new(mount).exists() {
                return Ok(());
            }
        }
        Err("Windows didn't eject the drive. Close any app using it and try again.".into())
    }
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

    /// Mount a scratch volume at FIVEFILES_EJECT_TEST (e.g. a disk image), then:
    /// `cargo test --lib eject_real -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn eject_real_volume() {
        let mount = std::env::var("FIVEFILES_EJECT_TEST").expect("set FIVEFILES_EJECT_TEST");
        let v = list().into_iter().find(|v| v.mount == mount).expect("volume listed");
        assert!(v.ejectable, "{v:?}");
        eject(&v.mount, &v.device).await.expect("eject");
        assert!(!std::path::Path::new(&mount).exists(), "still mounted");
        assert!(!list().iter().any(|v| v.mount == mount));
    }

    #[test]
    fn ejectable_volumes() {
        assert!(is_ejectable("macos", "/Volumes/USB", false));
        assert!(!is_ejectable("macos", "/", false));
        assert!(is_ejectable("linux", "/media/ana/SD", false));
        assert!(!is_ejectable("linux", "/home", false));
        assert!(is_ejectable("windows", "E:\\", true));
        assert!(!is_ejectable("windows", "D:\\", false));
    }

    #[test]
    fn lists_at_least_the_root_volume_and_home() {
        let v = list();
        assert!(!v.is_empty(), "expected at least one volume");
        assert!(v.iter().any(|v| v.total > 0));
        assert!(places().iter().any(|p| p.id == "home"));
    }
}
