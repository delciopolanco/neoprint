//! Per-user "start with the OS" switch, shared by the CLI installer, the
//! installers (Phase C) and the tray menu toggle.
//!
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value
//!   `Neoprint` = `"<exe>" --serve`. Replaces the old PowerShell-created
//!   Startup `.lnk` (kept only as a legacy cleanup). `--serve` skips the
//!   first-run self-installer, so an exe living outside `%LOCALAPPDATA%\Neoprint`
//!   (e.g. the MSI/Inno install dir) is never copied around.
//! - macOS: the per-user LaunchAgent written by `install.rs`.
//! - Other platforms: unsupported, always "off".

use std::path::Path;

use anyhow::Result;

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::bail;
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
        RRF_RT_REG_SZ,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Neoprint";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The command stored in the Run key for a given exe path.
    pub fn run_command(exe: &Path) -> String {
        format!("\"{}\" --serve", exe.display())
    }

    pub fn is_enabled() -> bool {
        let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
        // SAFETY: NUL-terminated buffers outlive the call; null out-params are
        // allowed by RegGetValueW when only existence is needed.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        status == 0
    }

    pub fn enable(exe: &Path) -> Result<()> {
        let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
        let data = wide(&run_command(exe));
        // SAFETY: buffers are NUL-terminated and outlive the call; the size is
        // in BYTES and includes the terminator, as REG_SZ requires.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if status != 0 {
            bail!("could not write the Run registry key (error {status})");
        }
        remove_legacy_startup_shortcut();
        Ok(())
    }

    pub fn disable() -> Result<()> {
        let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
        // SAFETY: as above. ERROR_FILE_NOT_FOUND (2) means already disabled.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        if status != 0 && status != 2 {
            bail!("could not remove the Run registry key (error {status})");
        }
        remove_legacy_startup_shortcut();
        Ok(())
    }

    /// Older builds created `Startup\Neoprint.lnk` via PowerShell. Remove it so
    /// the sidecar isn't launched twice (the 2nd instance would just exit, but
    /// there is no reason to keep the hack around).
    fn remove_legacy_startup_shortcut() {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let lnk = std::path::PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
                .join("Neoprint.lnk");
            let _ = std::fs::remove_file(lnk);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn run_command_quotes_the_path_and_serves() {
            let cmd = run_command(Path::new(r"C:\Users\Ana Pérez\AppData\Local\Neoprint\neoprint.exe"));
            assert_eq!(
                cmd,
                r#""C:\Users\Ana Pérez\AppData\Local\Neoprint\neoprint.exe" --serve"#
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn is_enabled() -> bool {
        crate::install::macos_plist_path()
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    pub fn enable(exe: &Path) -> Result<()> {
        crate::install::install_macos_launch_agent(exe)
    }

    pub fn disable() -> Result<()> {
        crate::install::remove_macos_launch_agent_if_exists()
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::*;

    pub fn is_enabled() -> bool {
        false
    }
    pub fn enable(_exe: &Path) -> Result<()> {
        anyhow::bail!("autostart is not supported on this platform")
    }
    pub fn disable() -> Result<()> {
        Ok(())
    }
}

pub use imp::{disable, enable, is_enabled};
