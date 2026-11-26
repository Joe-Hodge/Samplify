use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub fn register(app: &tauri::AppHandle) {
    let state = app.state::<crate::state::AppState>();
    let capture_hotkey = {
        let c = state.config.lock().unwrap();
        c.capture_hotkey.clone()
    };
    let gs = app.global_shortcut();

    if let Ok(sc) = capture_hotkey.parse::<Shortcut>() {
        let app2 = app.clone();
        let _ = gs.on_shortcut(sc, move |_a, _s, ev| {
            if ev.state == ShortcutState::Pressed { fire(&app2); }
        });
    }
    log::info!("Global shortcut registered: {capture_hotkey} (capture)");
}

pub fn re_register(app: &tauri::AppHandle, new_capture: &str) -> Result<(), String> {
    let capture_sc = new_capture.parse::<Shortcut>()
        .map_err(|_| format!("Invalid capture hotkey: {}", new_capture))?;

    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let app2 = app.clone();
    gs.on_shortcut(capture_sc, move |_a, _s, ev| {
        if ev.state == ShortcutState::Pressed { fire(&app2); }
    }).map_err(|e| format!("Failed to register capture hotkey: {}", e))?;

    log::info!("Global shortcut re-registered: {new_capture} (capture)");
    Ok(())
}

fn fire(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::commands::trigger_capture_cmd(app.clone()).await {
            log::error!("capture hotkey failed: {e}");
            let _ = tauri::Emitter::emit(&app, "mark-error", e);
        }
    });
}
