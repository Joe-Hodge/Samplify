use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub fn show_popup(app: &tauri::AppHandle, _mark_id: i64, _title: &str, _track_ms: i64) {
    // Reuse if already open.
    if let Some(w) = app.get_webview_window("popup") {
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit("popup-data", serde_json::json!({}));
        return;
    }

    let (x, y) = center_position(app, 900.0, 460.0);
    let _win = WebviewWindowBuilder::new(app, "popup", WebviewUrl::App("popup.html".into()))
        .title("Samplify — Capture")
        .inner_size(900.0, 460.0)
        .position(x, y)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(true)
        .build();
}

fn center_position(app: &tauri::AppHandle, w: f64, h: f64) -> (f64, f64) {
    if let Some(win) = app.get_webview_window("main").or_else(|| app.webview_windows().into_values().next()) {
        if let Ok(Some(mon)) = win.current_monitor() {
            let size = mon.size();
            let scale = mon.scale_factor();
            return ((size.width as f64 / scale - w) / 2.0, (size.height as f64 / scale - h) / 2.5);
        }
    }
    (400.0, 300.0)
}
