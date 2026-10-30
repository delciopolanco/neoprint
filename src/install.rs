//! Self-installer for the neoprint sidecar.
//!
//! Two supported platforms:
//!
//! - **Windows**: copies the binary to `%LOCALAPPDATA%\Neoprint` and
//!   drops a `.lnk` in the user's Startup folder. Zero external deps —
//!   the shortcut is created via PowerShell + `WScript.Shell` COM.
//! - **macOS**: copies the binary to `~/Library/Application Support/Neoprint`
//!   and registers a per-user LaunchAgent at
//!   `~/Library/LaunchAgents/com.neogeniux.neoprint.plist`. Activation
//!   uses `launchctl`.
//!
//! Linux + other Unixes fall through to a "not supported" notice with
//! exit code 2 so shell scripts can branch on the return code.
//!
//! CLI entry points (dispatched from `main.rs`):
//!   `neoprint --install`               → interactive prompt for autostart.
//!   `neoprint --install --autostart`   → non-interactive install + autostart.
//!   `neoprint --install --no-autostart`→ non-interactive install, no autostart.
//!   `neoprint --uninstall`             → remove autostart + installed binary.

use anyhow::Result;

/// Windows-only first-run hook. When the binary is launched with no
/// args from a location OTHER than `%LOCALAPPDATA%\Neoprint`, silently
/// self-install with autostart enabled, then return so `main()` can
/// drop through to serving. Idempotent: subsequent runs from the
/// install directory are a no-op. Skipped when
/// `NEOPRINT_SKIP_INSTALL=1` is set.
#[cfg(target_os = "windows")]
pub fn auto_install_on_first_run_if_needed() -> Result<()> {
    use anyhow::Context;
    use std::path::PathBuf;

    if std::env::var("NEOPRINT_SKIP_INSTALL").ok().as_deref() == Some("1") {
        return Ok(());
    }

    let Ok(local_appdata) = std::env::var("LOCALAPPDATA") else {
        return Ok(());
    };
    let install_dir = PathBuf::from(local_appdata).join("Neoprint");
    let install_target = install_dir.join("neoprint.exe");

    let current = std::env::current_exe().context("cannot resolve current exe path")?;

    if current == install_target {
        // Already running from the install location. Skip.
        return Ok(());
    }

    println!("neoprint first-run: installing to {}", install_dir.display());
    println!();

    // Non-interactive install with autostart. Matches the "double
    // click = installed + will boot on next login" mental model the
    // operator expects.
    run_install(Some(true))?;

    // There is no console window (windows subsystem): tell the cashier it
    // worked, in Spanish, with a message box.
    crate::console::notify_info(
        "neoprint",
        "neoprint se instaló y ahora se ejecuta en segundo plano. \
         Se iniciará solo cada vez que inicies sesión en Windows.",
    );

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn run_install(autostart: Option<bool>) -> Result<()> {
    use anyhow::Context;
    use std::path::PathBuf;

    let local_appdata = std::env::var("LOCALAPPDATA")
        .context("LOCALAPPDATA env var not set — cannot pick install directory")?;
    let install_dir = PathBuf::from(local_appdata).join("Neoprint");
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("failed to create {}", install_dir.display()))?;
    let dest = install_dir.join("neoprint.exe");

    let current = std::env::current_exe().context("cannot resolve current exe path")?;
    let is_installed_copy = current == dest;

    if is_installed_copy {
        println!("neoprint already runs from install location: {}", dest.display());
    } else {
        std::fs::copy(&current, &dest).with_context(|| {
            format!("failed to copy {} → {}", current.display(), dest.display())
        })?;
        println!("copied binary → {}", dest.display());
    }

    let want_autostart = match autostart {
        Some(v) => v,
        None => prompt_yes_no("Start neoprint automatically on Windows login?", true),
    };

    if want_autostart {
        crate::autostart::enable(&dest)?;
        println!("autostart enabled — Run registry key written");
    } else {
        crate::autostart::disable()?;
        println!("autostart NOT enabled — sidecar must be launched manually each session");
    }

    println!();
    println!("install complete.");
    println!("  binary:    {}", dest.display());
    println!("  autostart: {}", if want_autostart { "yes" } else { "no" });
    println!();
    println!("run now: \"{}\"", dest.display());

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn run_uninstall() -> Result<()> {
    use anyhow::Context;
    use std::path::PathBuf;

    crate::autostart::disable()?;

    let local_appdata = std::env::var("LOCALAPPDATA")
        .context("LOCALAPPDATA env var not set")?;
    let install_dir = PathBuf::from(local_appdata).join("Neoprint");
    let dest = install_dir.join("neoprint.exe");

    let current = std::env::current_exe().ok();
    let running_from_install = current.as_deref() == Some(dest.as_path());

    if running_from_install {
        println!(
            "note: uninstall was run from the installed binary — Windows keeps the file locked."
        );
        println!("      delete manually after this process exits: {}", install_dir.display());
    } else if dest.exists() {
        match std::fs::remove_file(&dest) {
            Ok(_) => println!("removed {}", dest.display()),
            Err(e) => println!("could not remove {} ({e}) — delete manually", dest.display()),
        }
        let _ = std::fs::remove_dir(&install_dir);
    }

    println!("uninstall complete.");
    Ok(())
}

#[cfg(target_os = "windows")]
fn prompt_yes_no(msg: &str, default_yes: bool) -> bool {
    use std::io::{stdin, stdout, Write};

    let hint = if default_yes { "Y/n" } else { "y/N" };
    print!("{msg} [{hint}] ");
    let _ = stdout().flush();

    let mut line = String::new();
    if stdin().read_line(&mut line).is_err() {
        return default_yes;
    }
    match line.trim().to_lowercase().as_str() {
        "" => default_yes,
        s if s.starts_with('y') => true,
        s if s.starts_with('n') => false,
        _ => default_yes,
    }
}

// ── macOS ─────────────────────────────────────────────────────────────
//
// Install target: `~/Library/Application Support/Neoprint/neoprint`.
// Autostart uses a per-user LaunchAgent plist under
// `~/Library/LaunchAgents/com.neogeniux.neoprint.plist` activated with
// `launchctl load`. Gatekeeper still blocks unsigned binaries on the
// first launch — README documents the right-click → Open workaround.

#[cfg(target_os = "macos")]
const MACOS_LAUNCH_LABEL: &str = "com.neogeniux.neoprint";

#[cfg(target_os = "macos")]
fn macos_install_dir() -> Result<std::path::PathBuf> {
    use anyhow::Context;
    let home = std::env::var("HOME").context("HOME env var not set")?;
    Ok(std::path::PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Neoprint"))
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_plist_path() -> Result<std::path::PathBuf> {
    use anyhow::Context;
    let home = std::env::var("HOME").context("HOME env var not set")?;
    Ok(std::path::PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{MACOS_LAUNCH_LABEL}.plist")))
}

#[cfg(target_os = "macos")]
pub fn auto_install_on_first_run_if_needed() -> Result<()> {
    use anyhow::Context;

    if std::env::var("NEOPRINT_SKIP_INSTALL").ok().as_deref() == Some("1") {
        return Ok(());
    }

    let install_target = macos_install_dir()?.join("neoprint");
    let current = std::env::current_exe().context("cannot resolve current exe path")?;
    if current == install_target {
        return Ok(());
    }

    println!("neoprint first-run: installing to {}", install_target.display());
    println!();
    run_install(Some(true))?;
    println!();
    println!("Sidecar will now start in THIS terminal. The LaunchAgent will boot it");
    println!("automatically on the next login.");
    println!();
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn run_install(autostart: Option<bool>) -> Result<()> {
    use anyhow::Context;
    use std::os::unix::fs::PermissionsExt;

    let install_dir = macos_install_dir()?;
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("failed to create {}", install_dir.display()))?;
    let dest = install_dir.join("neoprint");

    let current = std::env::current_exe().context("cannot resolve current exe path")?;
    if current != dest {
        std::fs::copy(&current, &dest).with_context(|| {
            format!("failed to copy {} → {}", current.display(), dest.display())
        })?;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", dest.display()))?;
        println!("copied binary → {}", dest.display());
    } else {
        println!("neoprint already runs from install location: {}", dest.display());
    }

    // Clear the quarantine attribute so Gatekeeper doesn't block the
    // LaunchAgent auto-launch. Non-fatal if xattr fails (already
    // cleared or the attribute never existed).
    let _ = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&dest)
        .status();

    let want_autostart = match autostart {
        Some(v) => v,
        None => prompt_yes_no("Start neoprint automatically on macOS login?", true),
    };

    if want_autostart {
        install_macos_launch_agent(&dest)?;
        println!("autostart enabled — LaunchAgent registered");
    } else {
        remove_macos_launch_agent_if_exists()?;
        println!("autostart NOT enabled — launch the binary manually each session");
    }

    println!();
    println!("install complete.");
    println!("  binary:    {}", dest.display());
    println!("  autostart: {}", if want_autostart { "yes" } else { "no" });
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn run_uninstall() -> Result<()> {
    remove_macos_launch_agent_if_exists()?;

    let install_dir = macos_install_dir()?;
    let dest = install_dir.join("neoprint");

    let current = std::env::current_exe().ok();
    let running_from_install = current.as_deref() == Some(dest.as_path());

    if running_from_install {
        println!("note: uninstall was run from the installed binary — delete manually after exit:");
        println!("      {}", install_dir.display());
    } else if dest.exists() {
        match std::fs::remove_file(&dest) {
            Ok(_) => println!("removed {}", dest.display()),
            Err(e) => println!("could not remove {} ({e}) — delete manually", dest.display()),
        }
        let _ = std::fs::remove_dir(&install_dir);
    }

    println!("uninstall complete.");
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_launch_agent(exe: &std::path::Path) -> Result<()> {
    use anyhow::Context;

    let plist_path = macos_plist_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let home = std::env::var("HOME").context("HOME env var not set")?;
    let logs_dir = std::path::PathBuf::from(&home).join("Library").join("Logs");
    let stdout_log = logs_dir.join("neoprint.log");
    let stderr_log = logs_dir.join("neoprint.err.log");
    // Best-effort: create the Logs directory. macOS ships with it, so
    // failure here is unusual and not fatal — launchd will surface the
    // missing path as a spawn error the user can grep for.
    let _ = std::fs::create_dir_all(&logs_dir);

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <!-- Restart only after a crash. A clean exit (tray "Quit") stays quit. -->
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>
"#,
        label = MACOS_LAUNCH_LABEL,
        exe = exe.display(),
        stdout = stdout_log.display(),
        stderr = stderr_log.display(),
    );

    std::fs::write(&plist_path, plist)
        .with_context(|| format!("failed to write {}", plist_path.display()))?;

    // Reload the agent. `unload` first to detach any previous version;
    // ignore its exit status because a first-time install has nothing
    // to unload.
    let _ = std::process::Command::new("launchctl")
        .arg("unload")
        .arg(&plist_path)
        .status();

    let status = std::process::Command::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(&plist_path)
        .status()
        .context("failed to spawn launchctl to load LaunchAgent")?;
    if !status.success() {
        anyhow::bail!("launchctl exited with status {status} while loading LaunchAgent");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn remove_macos_launch_agent_if_exists() -> Result<()> {
    let plist_path = macos_plist_path()?;
    if !plist_path.exists() {
        return Ok(());
    }
    let _ = std::process::Command::new("launchctl")
        .arg("unload")
        .arg(&plist_path)
        .status();
    std::fs::remove_file(&plist_path).ok();
    println!("removed LaunchAgent: {}", plist_path.display());
    Ok(())
}

#[cfg(target_os = "macos")]
fn prompt_yes_no(msg: &str, default_yes: bool) -> bool {
    use std::io::{stdin, stdout, Write};
    let hint = if default_yes { "Y/n" } else { "y/N" };
    print!("{msg} [{hint}] ");
    let _ = stdout().flush();
    let mut line = String::new();
    if stdin().read_line(&mut line).is_err() {
        return default_yes;
    }
    match line.trim().to_lowercase().as_str() {
        "" => default_yes,
        s if s.starts_with('y') => true,
        s if s.starts_with('n') => false,
        _ => default_yes,
    }
}

// ── Linux + other Unixes ─────────────────────────────────────────────
// No installer yet; the CLI commands print a friendly notice and exit
// non-zero so shell scripts can branch on the return code.

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn auto_install_on_first_run_if_needed() -> Result<()> {
    Ok(())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn run_install(_autostart: Option<bool>) -> Result<()> {
    eprintln!("--install is not supported on this platform yet.");
    eprintln!("Launch the binary directly or wire a systemd user unit.");
    std::process::exit(2);
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn run_uninstall() -> Result<()> {
    eprintln!("--uninstall is not supported on this platform yet.");
    std::process::exit(2);
}
