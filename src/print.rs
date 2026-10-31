//! Cross-platform print bridge.
//!
//! Two entry points:
//!   `send_raw(printer, bytes)` — dumps raw ESC/POS bytes at a printer.
//!     Used by thermal receipt commanda + fiscal-receipt encoders.
//!   `send_pdf(printer, pdf_bytes)` — hands a PDF file to the OS driver
//!     so inkjet / laser printers rasterize + print via their native
//!     driver.
//!
//! macOS + Linux use CUPS (`lp` / `lpr`). Windows uses the `printers`
//! crate for raw bytes and a bundled SumatraPDF for PDF (no Adobe / Edge
//! dependency, and no fallback that could open a PDF reader window).

use std::io::Write;

use anyhow::{Context, Result};
use printers::common::base::job::PrinterJobOptions;

/// What a printer's spooler accepts. `Escpos`: only raw ESC/POS bytes (a
/// generic / PostScript / text queue, or a thermal driver). `Pdf`: a driver
/// that renders a PDF. `Unknown`: the spooler reported no driver at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrinterLanguage {
    Escpos,
    Pdf,
    Unknown,
}

/// One printer as the OS spooler sees it, with the format it accepts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrinterDevice {
    pub name: String,
    pub language: PrinterLanguage,
}

/// Classify a spooler driver / make-and-model string. CUPS reports
/// `printer-make-and-model` (e.g. "Generic PostScript Printer" for a thermal
/// receipt printer installed as a generic queue, or "HP Smart Tank ... AirPrint"
/// for an inkjet); Windows reports the driver name (e.g. "Generic / Text Only").
/// A generic or raw queue cannot render a PDF, so it takes ESC/POS.
pub fn classify_language(driver: &str) -> PrinterLanguage {
    let d = driver.trim().to_ascii_lowercase();
    if d.is_empty() {
        return PrinterLanguage::Unknown;
    }
    // POS80 covers the Caysn POS80 driver used for 80mm receipt printers.
    const RAW_MARKERS: [&str; 9] = [
        "generic",
        "postscript",
        "raw",
        "text only",
        "thermal",
        "esc/pos",
        "escpos",
        "pos80",
        "caysn",
    ];
    if RAW_MARKERS.iter().any(|m| d.contains(m)) {
        PrinterLanguage::Escpos
    } else {
        PrinterLanguage::Pdf
    }
}

/// List every printer the OS spooler currently exposes, with its format.
pub fn list_devices() -> Vec<PrinterDevice> {
    printers::get_printers()
        .into_iter()
        .map(|p| PrinterDevice {
            language: classify_language(&p.driver_name),
            name: p.name,
        })
        .collect()
}

/// True when the OS spooler exposes a printer with this display or queue name.
/// Handlers validate the caller's name against this BEFORE it reaches `lp` /
/// PowerShell, so a crafted name can never be interpolated into a command.
pub fn printer_exists(name: &str) -> bool {
    printers::get_printers()
        .iter()
        .any(|p| p.name == name || p.system_name == name)
}

/// Send raw bytes to the named printer. Bypasses the driver's rendering
/// layer — printer must speak the same protocol the bytes encode
/// (typically ESC/POS on thermal receipt printers).
pub fn send_raw(printer_name: &str, bytes: &[u8]) -> Result<()> {
    let printers = printers::get_printers();
    let printer = printers
        .iter()
        .find(|p| p.name == printer_name)
        .with_context(|| format!("printer not found: {printer_name}"))?;
    let mut opts = PrinterJobOptions::none();
    opts.name = Some("neoprint raw");
    printer
        .print(bytes, opts)
        .map_err(|e| anyhow::anyhow!("raw print failed: {e:?}"))?;
    Ok(())
}

/// Print a PDF via the OS driver. Writes the bytes to a temp file so
/// `lp` / Adobe / SumatraPDF can pick them up.
pub fn send_pdf(printer_name: &str, pdf: &[u8]) -> Result<()> {
    sweep_old_temp_pdfs(&std::env::temp_dir(), STALE_PDF_AGE);
    let mut tmp = tempfile_pdf()?;
    tmp.write_all(pdf)?;
    tmp.flush()?;
    let path = tmp.path_owned();

    // Operator picked the human-readable display name from the sidecar
    // /printers list. macOS + Linux `lp -d` needs the CUPS queue name
    // (`system_name` on the Printer struct) which strips spaces + special
    // chars. Look up the matching printer + hand the correct identifier
    // to `lp`. Fall through with the raw input on Windows where the
    // display name IS the print-queue identifier.
    let resolved_name = resolve_system_name(printer_name);

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::process::Command;
        let status = Command::new("lp")
            .arg("-d")
            .arg(&resolved_name)
            .arg(&path)
            .status()
            .context("failed to invoke lp")?;
        if !status.success() {
            anyhow::bail!(
                "lp exited with status {status} (printer='{printer_name}', queue='{resolved_name}')"
            );
        }
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        // CREATE_NO_WINDOW: this process has no console, so without the flag
        // every child would flash a black console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // Bundled SumatraPDF (installed next to neoprint.exe) prints silently to
        // the named printer. There is deliberately NO fallback to the Windows
        // `PrintTo` verb: it launches Adobe Reader (or whatever app owns .pdf)
        // in the foreground when the printer is unavailable. Any failure here
        // returns an error instead, and neocrm falls back to the browser print
        // dialog.
        let sumatra = bundled_sumatra().context(
            "SumatraPDF.exe not found next to neoprint.exe (reinstall neoprint with its installer)",
        )?;
        let status = Command::new(&sumatra)
            .args(["-print-to", &resolved_name, "-silent", "-exit-when-done"])
            .arg(&path)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .context("could not run SumatraPDF")?;
        if !status.success() {
            anyhow::bail!("SumatraPDF exited with status {status} (printer='{printer_name}')");
        }
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!("unsupported platform"))
}

/// Match the caller's chosen display name back to the printer's OS-level
/// queue identifier. Falls back to the raw display name when no printer
/// matches so the caller still gets a useful `lp` / PrintTo error.
fn resolve_system_name(display_name: &str) -> String {
    for p in printers::get_printers() {
        if p.name == display_name || p.system_name == display_name {
            return p.system_name;
        }
    }
    display_name.to_string()
}

struct TmpFile {
    _file: std::fs::File,
    path: std::path::PathBuf,
}

impl TmpFile {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self._file.write_all(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self._file.flush()
    }
    fn path_owned(&self) -> std::path::PathBuf {
        self.path.clone()
    }
}

fn tempfile_pdf() -> Result<TmpFile> {
    // Custom tempfile: `NamedTempFile` from the `tempfile` crate would
    // work but we want the path stable while the OS reader spawns +
    // reads it. Roll our own so the file survives the `Drop` until
    // print returns.
    let mut path = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    path.push(format!("neoprint-{stamp}.pdf"));
    let file = std::fs::File::create(&path)?;
    Ok(TmpFile { _file: file, path })
}

/// Temp PDFs older than this are deleted on the next PDF print. They are not
/// removed right after printing because a PDF app may still be reading the file
/// when its launcher returns.
const STALE_PDF_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Best-effort cleanup of `neoprint-*.pdf` files older than `max_age` in `dir`.
fn sweep_old_temp_pdfs(dir: &std::path::Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("neoprint-") && name.ends_with(".pdf")) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= max_age);
        if old_enough {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `SumatraPDF.exe` shipped beside `neoprint.exe` by the Windows installer.
#[cfg(target_os = "windows")]
fn bundled_sumatra() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("SumatraPDF.exe");
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn generic_postscript_queue_takes_escpos() {
        assert_eq!(
            classify_language("Generic PostScript Printer"),
            PrinterLanguage::Escpos
        );
        assert_eq!(classify_language("Generic / Text Only"), PrinterLanguage::Escpos);
        assert_eq!(classify_language("Star TSP100 Thermal"), PrinterLanguage::Escpos);
        assert_eq!(classify_language("Caysn POS80, 1.0"), PrinterLanguage::Escpos);
    }

    #[test]
    fn driver_printers_take_pdf_and_missing_driver_is_unknown() {
        assert_eq!(
            classify_language("HP Smart Tank 580-590 series-AirPrint"),
            PrinterLanguage::Pdf
        );
        assert_eq!(classify_language("Microsoft Print to PDF"), PrinterLanguage::Pdf);
        assert_eq!(classify_language("  "), PrinterLanguage::Unknown);
    }

    #[test]
    fn sweep_removes_only_stale_neoprint_pdfs() {
        let dir = std::env::temp_dir().join(format!("neoprint-sweep-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("neoprint-1.pdf");
        let other = dir.join("report.pdf");
        std::fs::write(&stale, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();

        // Everything is "newer than 1h": nothing is removed.
        sweep_old_temp_pdfs(&dir, Duration::from_secs(3600));
        assert!(stale.exists() && other.exists());

        // max_age 0: every neoprint-*.pdf is stale, unrelated files are never touched.
        sweep_old_temp_pdfs(&dir, Duration::ZERO);
        assert!(!stale.exists(), "stale neoprint pdf must be deleted");
        assert!(other.exists(), "unrelated files must be left alone");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
