//! System-tray / menu-bar presence: the "QZ Tray" part. The sidecar is a
//! background app, so the tray icon is the operator's only handle on it —
//! status, logs, restart, autostart toggle, quit.
//!
//! The event loop must own the main thread on both Windows and macOS; the HTTP
//! server runs on a separate thread (see `main.rs`).

use std::path::PathBuf;

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::{autostart, console, logging, update};

const TOOLTIP: &str = "neoprint";
const LABEL_OPEN_LOGS: &str = "Abrir carpeta de registros";
const LABEL_RESTART: &str = "Reiniciar neoprint";
const LABEL_AUTOSTART: &str = "Iniciar con el sistema";
const LABEL_QUIT: &str = "Salir de neoprint";
const MSG_AUTOSTART_FAILED: &str = "No se pudo cambiar el inicio automático.";

const LABEL_UPDATE_NONE: &str = "Buscando actualizaciones…";
const LABEL_UPDATE_UPTODATE: &str = "neoprint está al día";
const MSG_UPDATE_FAILED: &str = "No se pudo actualizar neoprint.";
/// How often the tray re-checks for a newer version.
const UPDATE_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

enum UserEvent {
    Menu(MenuEvent),
    /// Result of an update check: `Some` when a newer installer is available.
    Update(Option<update::Manifest>),
}

/// Teal disc with a white printer glyph, drawn in code so the binary needs no
/// asset file. Swap for a branded `.ico`/PNG via `Icon::from_rgba` later.
fn icon() -> Icon {
    const N: u32 = 32;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let center = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - center, y as f32 - center);
            let inside = dx * dx + dy * dy <= 15.0 * 15.0;
            // printer: paper (top), body (middle), tray (bottom)
            let paper = (11..=20).contains(&x) && (7..=12).contains(&y);
            let body = (7..=24).contains(&x) && (13..=20).contains(&y);
            let tray = (10..=21).contains(&x) && (18..=25).contains(&y);
            let px = ((y * N + x) * 4) as usize;
            if inside {
                let glyph = paper || body || tray;
                let (r, g, b) = if glyph { (255, 255, 255) } else { (15, 118, 110) };
                rgba[px..px + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
    Icon::from_rgba(rgba, N, N).expect("32x32 RGBA buffer is valid")
}

fn open_in_file_manager(dir: &PathBuf) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    let _ = std::process::Command::new(opener).arg(dir).spawn();
}

/// Relaunch ourselves. The new process waits for this one to release the port
/// (`NEOPRINT_RESTART=1`), so spawn first, then exit.
fn restart() -> ! {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .arg("--serve")
            .env("NEOPRINT_RESTART", "1")
            .spawn();
    }
    std::process::exit(0);
}

/// Run the tray event loop on the calling (main) thread. Never returns.
pub fn run(status_line: String) -> ! {
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        // Menu-bar only: no Dock icon, no app switcher entry.
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    let status = MenuItem::new(status_line, false, None);
    let logs = MenuItem::new(LABEL_OPEN_LOGS, true, None);
    let restart_item = MenuItem::new(LABEL_RESTART, true, None);
    let auto = CheckMenuItem::new(LABEL_AUTOSTART, true, autostart::is_enabled(), None);
    let quit = MenuItem::new(LABEL_QUIT, true, None);
    // Disabled until a check finds something; text reflects the check state.
    let update_item = MenuItem::new(
        if update::manifest_url().is_some() { LABEL_UPDATE_NONE } else { LABEL_UPDATE_UPTODATE },
        false,
        None,
    );

    let menu = Menu::new();
    let _ = menu.append_items(&[
        &status,
        &update_item,
        &PredefinedMenuItem::separator(),
        &logs,
        &auto,
        &restart_item,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);

    let ids: [MenuId; 5] = [
        logs.id().clone(),
        restart_item.id().clone(),
        auto.id().clone(),
        quit.id().clone(),
        update_item.id().clone(),
    ];

    if let Some(url) = update::manifest_url() {
        spawn_update_checker(url, event_loop.create_proxy());
    }
    let mut pending_update: Option<update::Manifest> = None;

    let mut tray = None;
    let mut menu = Some(menu);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // The tray icon must be created once the loop is running (macOS).
            Event::NewEvents(StartCause::Init) => {
                match TrayIconBuilder::new()
                    .with_menu(Box::new(menu.take().expect("menu built once")))
                    .with_tooltip(TOOLTIP)
                    .with_icon(icon())
                    .build()
                {
                    Ok(t) => tray = Some(t),
                    Err(err) => tracing::error!(%err, "could not create the tray icon"),
                }
            }
            Event::UserEvent(UserEvent::Update(found)) => {
                match &found {
                    Some(m) => {
                        update_item.set_text(format!("Actualizar a v{}", m.version));
                        update_item.set_enabled(true);
                    }
                    None => {
                        update_item.set_text(LABEL_UPDATE_UPTODATE);
                        update_item.set_enabled(false);
                    }
                }
                pending_update = found;
            }
            Event::UserEvent(UserEvent::Menu(ev)) => {
                let id = &ev.id;
                if *id == ids[0] {
                    open_in_file_manager(&logging::log_dir());
                } else if *id == ids[1] {
                    restart();
                } else if *id == ids[2] {
                    let want = auto.is_checked();
                    let exe = std::env::current_exe().ok();
                    let result = match (want, exe) {
                        (true, Some(exe)) => autostart::enable(&exe),
                        (true, None) => Err(anyhow::anyhow!("cannot resolve the exe path")),
                        (false, _) => autostart::disable(),
                    };
                    if let Err(err) = result {
                        tracing::error!(%err, "autostart toggle failed");
                        console::notify_error(TOOLTIP, &format!("{MSG_AUTOSTART_FAILED}\n\n{err:#}"));
                    }
                    // Reflect what is actually registered, not what was clicked.
                    auto.set_checked(autostart::is_enabled());
                } else if *id == ids[4] {
                    if let Some(asset) = pending_update.as_ref().map(|m| m.asset()) {
                        update_item.set_enabled(false);
                        update_item.set_text("Descargando actualización…");
                                                // Download off the UI thread; on success the installer
                        // takes over (and stops this process on Windows).
                        std::thread::spawn(move || {
                            if let Err(err) = update::download_and_run(&asset) {
                                tracing::error!(%err, "update failed");
                                console::notify_error(TOOLTIP, &format!("{MSG_UPDATE_FAILED}\n\n{err:#}"));
                            }
                        });
                    }
                } else if *id == ids[3] {
                    tracing::info!("quit requested from the tray");
                    drop(tray.take());
                    std::process::exit(0);
                }
            }
            _ => {}
        }
    })
}

/// Poll the manifest now and then. Failures are logged and retried next round:
/// a terminal without internet must never see an error box.
fn spawn_update_checker(
    manifest_url: &'static str,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) {
    std::thread::spawn(move || loop {
        match update::check(manifest_url, env!("CARGO_PKG_VERSION")) {
            Ok(found) => {
                match &found {
                    Some(m) => tracing::info!(version = %m.version, "update available"),
                    None => tracing::info!("neoprint is up to date"),
                }
                let _ = proxy.send_event(UserEvent::Update(found));
            }
            Err(err) => tracing::warn!(%err, "update check failed"),
        }
        std::thread::sleep(UPDATE_CHECK_EVERY);
    });
}
