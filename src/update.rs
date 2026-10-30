//! Self-update (Phase F). The tray checks a small manifest published next to
//! the installers and, when a newer version exists, offers "Actualizar".
//!
//! Manifest: one small file PER PLATFORM next to the installer
//! (`neoprint/latest/version-windows.json`, `version-macos.json`), written by the
//! job that builds that installer, so the two publishers never race:
//! ```json
//! { "version": "0.2.0", "url": "https://…/neoprint-setup.exe", "sha256": "…" }
//! ```
//! The manifest URL is baked in at build time (`NEOPRINT_UPDATE_URL`, set by
//! CI); builds without it never check. The download is verified against the
//! manifest's SHA-256 before it runs. That guards against corruption; the trust
//! root for authenticity is HTTPS + the installer's code signature.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Manifest location, fixed at compile time. `None` disables update checks.
pub const MANIFEST_URL: Option<&str> = option_env!("NEOPRINT_UPDATE_URL");

/// Set `NEOPRINT_DISABLE_UPDATE_CHECK=1` to make neoprint never contact the
/// update server.
pub const ENV_DISABLE: &str = "NEOPRINT_DISABLE_UPDATE_CHECK";

/// The manifest URL to poll, or `None` when this build has none or the operator
/// opted out.
pub fn manifest_url() -> Option<&'static str> {
    if std::env::var(ENV_DISABLE).ok().as_deref() == Some("1") {
        return None;
    }
    MANIFEST_URL
}

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_INSTALLER_BYTES: u64 = 200 * 1024 * 1024;

/// What to download and how to verify it.
#[derive(Debug, Clone)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

impl Manifest {
    pub fn asset(&self) -> Asset {
        Asset {
            url: self.url.clone(),
            sha256: self.sha256.clone(),
        }
    }
}

/// HTTP agent using the operating system's TLS stack.
fn agent() -> Result<ureq::Agent> {
    let tls = native_tls::TlsConnector::new().context("could not initialise TLS")?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .build())
}

/// `major.minor.patch` (extra suffixes after `-`/`+` ignored). `None` when it
/// does not parse, so a malformed manifest can never trigger an "update".
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

/// Fetch the manifest; `Some` only when it advertises a newer version.
pub fn check(manifest_url: &str, current: &str) -> Result<Option<Manifest>> {
    let manifest: Manifest = agent()?
        .get(manifest_url)
        .timeout(HTTP_TIMEOUT)
        .call()
        .context("could not fetch the update manifest")?
        .into_json()
        .context("the update manifest is not valid JSON")?;
    if is_newer(&manifest.version, current) {
        Ok(Some(manifest))
    } else {
        Ok(None)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download the installer, verify its hash, and launch it. The process exits
/// right after on Windows (the silent installer replaces neoprint.exe and
/// restarts it); on macOS the .pkg opens in Installer.app.
pub fn download_and_run(asset: &Asset) -> Result<()> {
    if !asset.url.starts_with("https://") {
        bail!("refusing to download an installer over a non-HTTPS URL");
    }
    let mut bytes = Vec::new();
    agent()?
        .get(&asset.url)
        .timeout(Duration::from_secs(300))
        .call()
        .context("could not download the installer")?
        .into_reader()
        .take(MAX_INSTALLER_BYTES)
        .read_to_end(&mut bytes)
        .context("download interrupted")?;

    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(asset.sha256.trim()) {
        bail!("installer hash mismatch (expected {}, got {actual})", asset.sha256);
    }

    #[cfg(windows)]
    let file = "neoprint-update.exe";
    #[cfg(not(windows))]
    let file = "neoprint-update.pkg";
    let path = std::env::temp_dir().join(file);
    std::fs::write(&path, &bytes).context("could not save the installer")?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW; the installer stops this process, swaps the exe
        // and relaunches neoprint (its [Run] section) on its own.
        std::process::Command::new(&path)
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
            .creation_flags(0x0800_0000)
            .spawn()
            .context("could not start the installer")?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .context("could not open the installer")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_compare_numerically_not_lexically() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn malformed_versions_never_trigger_an_update() {
        for bad in ["", "latest", "1", "1.2", "1.2.3.4", "a.b.c"] {
            assert!(!is_newer(bad, "0.1.0"), "{bad:?}");
        }
        assert!(!is_newer("0.2.0", "garbage"));
    }

    #[test]
    fn prerelease_suffix_is_ignored_for_ordering() {
        assert!(is_newer("0.2.0-beta.1", "0.1.0"));
    }

    #[test]
    fn manifest_parses_and_hash_is_lowercase_hex() {
        let m: Manifest = serde_json::from_str(
            r#"{"version":"0.2.0","url":"https://x/y.exe","sha256":"ab"}"#,
        )
        .unwrap();
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.asset().url, "https://x/y.exe");
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
