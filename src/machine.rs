//! Stable identity of this computer, for per-terminal print settings.
//!
//! The OS machine id (macOS `IOPlatformUUID`, Windows `MachineGuid`, Linux
//! `/etc/machine-id`) is read without admin rights and never leaves this
//! process: only a salted SHA-256 of it is reported, as `machineId`. The
//! hostname is reported for display only.

use sha2::{Digest, Sha256};
use std::process::Command;
use std::sync::OnceLock;

/// Versioned salt: changing it re-keys every terminal, so keep it stable.
const SALT: &str = "neoprint-machine-id/v1";

#[derive(Debug, PartialEq, Eq)]
pub struct MachineIdentity {
    pub machine_id: String,
    pub hostname: String,
}

static IDENTITY: OnceLock<Option<MachineIdentity>> = OnceLock::new();

/// This computer's identity, read once. `None` when the OS id cannot be read.
pub fn identity() -> Option<&'static MachineIdentity> {
    IDENTITY
        .get_or_init(|| {
            read_raw_id().map(|raw| MachineIdentity {
                machine_id: hash_machine_id(&raw),
                hostname: hostname(),
            })
        })
        .as_ref()
}

/// Salted SHA-256 of the raw OS id, as 64 lowercase hex characters.
pub fn hash_machine_id(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SALT.as_bytes());
    hasher.update([0u8]);
    hasher.update(raw.trim().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_raw_id() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()?;
        parse_ioreg_uuid(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(target_os = "windows")]
    {
        let out = Command::new("reg")
            .args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
            .output()
            .ok()?;
        parse_reg_guid(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/etc/machine-id").ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// Pulls `IOPlatformUUID` out of `ioreg` output: `"IOPlatformUUID" = "<uuid>"`.
pub fn parse_ioreg_uuid(text: &str) -> Option<String> {
    text.lines()
        .find(|line| line.contains("IOPlatformUUID"))
        .and_then(|line| line.split('=').nth(1))
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
}

/// Pulls `MachineGuid` out of `reg query` output: the last token on its line.
pub fn parse_reg_guid(text: &str) -> Option<String> {
    text.lines()
        .find(|line| line.contains("MachineGuid"))
        .and_then(|line| line.split_whitespace().last())
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

fn hostname() -> String {
    #[cfg(target_os = "windows")]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Equipo".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("hostname")
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Equipo".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_hex_and_keyed_on_the_raw_id() {
        let a = hash_machine_id("2A3F-UUID");
        assert_eq!(a, hash_machine_id("2A3F-UUID"));
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, hash_machine_id("OTHER-UUID"));
        // Surrounding whitespace from the OS tools must not change the id.
        assert_eq!(a, hash_machine_id("  2A3F-UUID\n"));
        // The raw id is never part of the output.
        assert!(!a.contains("2A3F"));
    }

    #[test]
    fn parses_the_ioreg_platform_uuid() {
        let sample = "    \"IOPlatformSerialNumber\" = \"C02X\"\n    \"IOPlatformUUID\" = \"6A3B2C1D-1111-2222-3333-444455556666\"\n";
        assert_eq!(
            parse_ioreg_uuid(sample).as_deref(),
            Some("6A3B2C1D-1111-2222-3333-444455556666")
        );
        assert_eq!(parse_ioreg_uuid("no uuid here"), None);
    }

    #[test]
    fn parses_the_windows_machine_guid() {
        let sample = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n    MachineGuid    REG_SZ    9f1c2d3e-aaaa-bbbb-cccc-0123456789ab\r\n";
        assert_eq!(
            parse_reg_guid(sample).as_deref(),
            Some("9f1c2d3e-aaaa-bbbb-cccc-0123456789ab")
        );
        assert_eq!(parse_reg_guid("ERROR: The system was unable to find the value"), None);
    }
}
