//! macOS privacy (TCC). Without Full Disk Access, macOS asks for permission separately for
//! Documents, Desktop, Downloads, removable and network volumes — and a file manager that
//! reads many folders at once can trigger a burst of identical prompts. With Full Disk
//! Access granted once in System Settings, none of them appear.

/// `None` where it doesn't apply (Windows, Linux); otherwise whether 5files has Full Disk
/// Access. Probed by reading places only it opens (this never shows a prompt).
pub fn full_disk_access() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        // Places only Full Disk Access opens (none of them ever prompt). Their layout
        // differs between macOS versions, so: any readable means yes, any refused means no.
        let home = dirs::home_dir()?;
        let probes = [
            std::path::PathBuf::from("/Library/Application Support/com.apple.TCC/TCC.db"),
            home.join("Library/Application Support/com.apple.TCC/TCC.db"),
            home.join("Library/Safari"),
            home.join("Library/Mail"),
            home.join("Library/Messages"),
        ];
        let mut refused = false;
        for p in &probes {
            let r = if p.is_dir() { std::fs::read_dir(p).map(|_| ()) } else { std::fs::File::open(p).map(|_| ()) };
            match r {
                Ok(()) => return Some(true),
                Err(e) if matches!(e.raw_os_error(), Some(1) | Some(13)) => refused = true,
                Err(_) => {}
            }
        }
        Some(!refused)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Opens System Settings at Privacy & Security → Full Disk Access.
pub fn open_full_disk_access_settings() -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles").status()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn probe_answers_where_it_applies() {
        let r = super::full_disk_access();
        assert_eq!(r.is_some(), cfg!(target_os = "macos"));
    }
}
