//! neoprint — silent-print sidecar for the neogeniux POS + restaurant flows.
//!
//! Binds an HTTP server on `127.0.0.1:9787`, exposes a small JSON API that
//! neocrm's browser tab POSTs to. Forwards raw ESC/POS bytes or rendered
//! PDF blobs to the OS printer spooler. Replaces the QZ Tray licensed
//! bridge — no per-page authorization prompt, no license fee, no external
//! signing dependency.
//!
//! Endpoints
//!   GET  /health              → readiness probe (`{"ok": true}`).
//!   GET  /printers            → `{"printers": ["<name>", ...]}`.
//!   POST /print/raw           → `{ "printer", "bytesBase64" }`.
//!   POST /print/pdf           → `{ "printer", "pdfBase64" }`.
//!
//! Auth
//!   Every request must carry an `X-Neoprint-Token` header matching the
//!   token generated on first boot (see `token::load_or_create`). Rogue
//!   browser tabs on the same machine can't drive prints without it.
//!
//! Persistence
//!   Config lives at:
//!     macOS/Linux: `$HOME/.neoprint/config.toml`
//!     Windows:     `%APPDATA%\neoprint\config.toml`
//!   Token is generated on first boot; the operator pastes it into
//!   Configuración → Impresoras on neocrm.

// Background process: no console window on Windows. See `console.rs` for how
// CLI output and first-run / fatal-error feedback work without one.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::time::Duration;

mod autostart;
mod console;
mod http;
mod install;
mod logging;
mod machine;
mod origin;
mod print;
#[cfg(any(windows, target_os = "macos"))]
mod tray;
mod update;

/// Loopback address the sidecar serves on. Hardcoded here + on the FE
/// dispatcher (`print-tauri.ts`); change both together.
const LISTEN_ADDR: &str = "127.0.0.1:9787";

const APP_TITLE: &str = "neoprint";
const MSG_ALREADY_RUNNING: &str =
    "neoprint ya está en ejecución en segundo plano. No hace falta abrirlo de nuevo.";
const MSG_FATAL: &str = "neoprint no pudo iniciar.";

fn main() {
    // CLI flags print to the console that launched us; a double-click has none.
    if std::env::args().len() > 1 {
        console::attach_parent_console();
    }

    // Keep the guard alive for the whole process so the log file is flushed.
    let _log_guard = match startup() {
        Ok(guard) => guard,
        Err(err) => fatal(&err),
    };

    spawn_server();

    #[cfg(any(windows, target_os = "macos"))]
    tray::run(format!(
        "neoprint v{} · {LISTEN_ADDR}",
        env!("CARGO_PKG_VERSION")
    ));

    // Headless platforms (Linux): no tray, just keep the process alive.
    #[cfg(not(any(windows, target_os = "macos")))]
    loop {
        std::thread::park();
    }
}

fn fatal(err: &anyhow::Error) -> ! {
    tracing::error!(error = %err, "neoprint failed");
    eprintln!("neoprint: {err:#}");
    console::notify_error(APP_TITLE, &format!("{MSG_FATAL}\n\n{err:#}"));
    std::process::exit(1);
}

/// Everything that must happen before the server starts. Returns the log
/// guard, or exits the process for one-shot CLI commands / a second instance.
fn startup() -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    if let Some(exit_code) = dispatch_cli()? {
        std::process::exit(exit_code);
    }

    let log_guard = logging::init();

    // After a tray "Restart" the old instance is still releasing the port.
    if std::env::var("NEOPRINT_RESTART").ok().as_deref() == Some("1") {
        for _ in 0..50 {
            if !already_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    // A second launch (double-click on an already-running sidecar) must not
    // try to re-install over the running exe or crash on the busy port.
    if already_running() {
        tracing::info!("another neoprint instance already serves {LISTEN_ADDR}; exiting");
        console::notify_info(APP_TITLE, MSG_ALREADY_RUNNING);
        // The info box lives on its own thread; keep the process up until the
        // operator has had a chance to read it.
        #[cfg(windows)]
        std::thread::sleep(Duration::from_secs(8));
        std::process::exit(0);
    }

    // Windows double-click UX: when the binary boots with no args from
    // a "not installed" location (Downloads, temp, USB stick, …),
    // treat the launch as an implicit install request. Copies the exe
    // to `%LOCALAPPDATA%\Neoprint`, registers autostart, and then
    // drops through to serve so the sidecar is ALSO usable in the
    // current session. Advanced users can opt out via
    // `NEOPRINT_SKIP_INSTALL=1` or by passing `--serve`.
    install::auto_install_on_first_run_if_needed()?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "neoprint startup (loopback, origin-allowlisted)"
    );
    Ok(log_guard)
}

/// Serve the HTTP API on a background thread; the main thread belongs to the
/// tray event loop. A server failure (e.g. port taken in the race window) is
/// fatal and surfaced to the operator.
fn spawn_server() {
    std::thread::Builder::new()
        .name("neoprint-http".into())
        .spawn(|| {
            let result = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(serve()));
            if let Err(err) = result {
                fatal(&err);
            }
        })
        .expect("failed to spawn the HTTP server thread");
}

async fn serve() -> anyhow::Result<()> {
    let app = http::router();

    // Loopback bind so no LAN neighbor can drive prints.
    let addr: SocketAddr = LISTEN_ADDR.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app).await?;
    Ok(())
}

/// True when something already accepts connections on the sidecar port.
fn already_running() -> bool {
    LISTEN_ADDR
        .parse::<SocketAddr>()
        .ok()
        .and_then(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok())
        .is_some()
}

/// Parse `std::env::args` and, if the caller asked for a one-shot
/// subcommand (`--install`, `--uninstall`, `--help`, `--version`), run it
/// and return `Some(exit_code)` so `main` can exit before booting the
/// HTTP server. Returns `Ok(None)` when the process should continue to
/// serve.
fn dispatch_cli() -> anyhow::Result<Option<i32>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Ok(None);
    }

    match args[0].as_str() {
        "--install" | "install" => {
            let autostart = if args.iter().any(|a| a == "--autostart") {
                Some(true)
            } else if args.iter().any(|a| a == "--no-autostart") {
                Some(false)
            } else {
                None
            };
            install::run_install(autostart)?;
            Ok(Some(0))
        }
        "--uninstall" | "uninstall" => {
            install::run_uninstall()?;
            Ok(Some(0))
        }
        "--help" | "-h" | "help" => {
            print_usage();
            Ok(Some(0))
        }
        "--version" | "-V" => {
            println!("neoprint {}", env!("CARGO_PKG_VERSION"));
            Ok(Some(0))
        }
        "--serve" | "serve" => {
            // Explicit opt-in to serve without triggering the
            // first-run auto-installer. Used by advanced operators
            // who want to keep the binary in-place.
            std::env::set_var("NEOPRINT_SKIP_INSTALL", "1");
            Ok(None)
        }
        other => {
            eprintln!("neoprint: unknown argument '{other}'");
            print_usage();
            Ok(Some(2))
        }
    }
}

fn print_usage() {
    println!(
        "neoprint — silent-print sidecar ({})",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("USAGE:");
    println!("  neoprint                          first run from a non-install path: auto-install + autostart, then serve");
    println!("                                    subsequent runs from install path: serve only");
    println!(
        "  neoprint --serve                  serve without triggering the first-run installer"
    );
    println!("  neoprint --install                install to %LOCALAPPDATA%\\Neoprint (prompts for autostart)");
    println!("  neoprint --install --autostart    install + register Startup-folder shortcut");
    println!("  neoprint --install --no-autostart install without autostart");
    println!("  neoprint --uninstall              remove startup shortcut + installed binary");
    println!("  neoprint --version                print version");
    println!("  neoprint --help                   print this message");
    println!();
    println!("Install / uninstall are Windows-only. On macOS + Linux launch the binary directly.");
}
