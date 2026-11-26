// Platform-dispatched helpers for the two OS interactions that have no
// cross-platform stdlib equivalent: opening a URL in the default handler
// and revealing a file in the OS file manager. Both used to be inline
// Windows-only shell-outs in `commands.rs`; this module is the single
// place to read them.
//
// Pattern: a `#[cfg(target_os = "...")]` per platform — no dyn trait, because
// there is exactly one backend per build (same shape as `capture.rs`).

use anyhow::Result;
use std::path::Path;

/// Open `url` in the OS-default handler (the user's default browser).
///
/// Windows: PowerShell `Start-Process '<url>'` — the original choice.
/// Quoting is safe here because Samplify only builds OAuth/Spotify URLs that
/// are already percent-encoded (`cmd /C start` would otherwise mangle `&`).
///
/// macOS: /usr/bin/open, the system's standard "open with default app" tool.
pub fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("powershell")
            .args(["-Command", &format!("Start-Process '{}'", url)])
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()?;
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = url;
        Err(anyhow::anyhow!("unsupported platform for opening URL"))
    }
}

/// Reveal `path` in the OS file manager, selecting the file if possible.
///
/// Windows: `explorer /select,"PATH"` via `raw_arg` so paths with spaces /
/// brackets survive the quoting that `explorer /select,PATH` mangles.
///
/// macOS: `open -R '<path>'` opens the containing folder in Finder with the
/// file selected (Finder focuses the file itself, not just the folder).
pub fn reveal_path(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-R", &path.to_string_lossy()])
            .spawn()?;
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = path;
        Err(anyhow::anyhow!("unsupported platform for revealing files"))
    }
}

/// Build a `Command` for an external program without flashing a console
/// window. On Windows this attaches the `CREATE_NO_WINDOW` creation flag
/// (0x08000000). On macOS no flag is needed — GUI-spawned processes have no
/// console to suppress.
pub fn silent_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    c
}

/// Open System Settings to the Screen Recording privacy pane (macOS only).
/// No-op on Windows (WASAPI loopback needs no permission). Returns Ok on
/// Windows; macOS uses the `x-apple.systempreferences:` URL scheme which
/// macOS's `open` resolves directly to the System Settings app.
pub fn open_screen_recording_settings() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
            .spawn()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        // No-op on Windows — WASAPI loopback needs no permission grant.
    }
    Ok(())
}