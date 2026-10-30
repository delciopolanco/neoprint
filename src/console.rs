//! User feedback for a process that has no console window.
//!
//! The Windows build uses the `windows` subsystem so the sidecar runs in the
//! background without a black console window. That also detaches stdout, so:
//!  - CLI flags (`--install`, `--help`, …) re-attach to the parent console
//!    (cmd / PowerShell) to keep printing there;
//!  - first-run success and fatal errors surface as a message box, because a
//!    cashier who double-clicked the exe has no terminal to read.
//!
//! On macOS / Linux these are no-ops (stdout is the terminal or a LaunchAgent
//! log file).

/// Re-attach to the console of the process that launched us, if any. No-op
/// when started by double-click (there is no parent console to attach to).
#[cfg(windows)]
pub fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: plain Win32 call, no pointers; failure just means "no console".
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
pub fn attach_parent_console() {}

#[cfg(windows)]
fn message_box(title: &str, body: &str, icon: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (title, body) = (wide(title), wide(body));
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | icon,
        );
    }
}

/// Informational notice. Shown on its own thread so the (modal) box never
/// delays the server from starting.
#[cfg(windows)]
pub fn notify_info(title: &str, body: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION;
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || message_box(&title, &body, MB_ICONINFORMATION));
}

/// Fatal error. Blocks until dismissed: the process exits right after.
#[cfg(windows)]
pub fn notify_error(title: &str, body: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR;
    message_box(title, body, MB_ICONERROR);
}

#[cfg(not(windows))]
pub fn notify_info(title: &str, body: &str) {
    println!("{title}: {body}");
}

#[cfg(not(windows))]
pub fn notify_error(title: &str, body: &str) {
    eprintln!("{title}: {body}");
}
