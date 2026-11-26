mod bridge;
mod capture;
#[cfg(any(windows, test))]
mod spotify_buffer;
#[cfg(target_os = "windows")]
mod capture_win;
#[cfg(target_os = "macos")]
mod capture_mac;
mod clock;
mod commands;
mod hotkeys;
mod platform;
mod playground;
mod popup;
mod session;
mod spotify;
mod state;
mod stems;
mod store;

use state::{AppState, Config};
use std::sync::{Arc, Mutex};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _a, _c| {
            if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_drag::init())
        .setup(|app| {
            let data_dir = dirs::data_dir().unwrap().join("com.samplify.app");
            let store = store::Store::open(&data_dir.join("samplify.db"))?;
            let config = Config::load(&store);
            #[cfg(windows)]
            let (sr, ch) = (capture_win::RATE, capture_win::CHANNELS);
            #[cfg(target_os = "macos")]
            let (sr, ch) = capture_mac::device_format().unwrap_or((48000, 2));
            let ring = Arc::new(Mutex::new(capture::RingBuffer::new(sr, ch, config.buffer_seconds)));
            #[cfg(windows)]
            let desktop_audio = capture_win::WindowsAudio::start(ring.clone(), config.buffer_seconds, config.system_capture)?;
            #[cfg(target_os = "macos")]
            match capture_mac::start(ring.clone()) {
                Ok(engine) => { app.manage(engine); }
                Err(e) => log::error!("ScreenCaptureKit could not start: {e}"),
            }
            let tokens = Mutex::new(spotify::load_tokens(&store));
            app.manage(AppState {
                #[cfg(windows)]
                desktop_audio,
                store, clock: Mutex::new(clock::Clock::default()), ring,
                tokens, config: Mutex::new(config),
                bridge_track: Mutex::new(None),
                auth_in_progress: std::sync::atomic::AtomicBool::new(false),
            });
            // Drag slices are disposable — clear last run's before we make more.
            // Best-effort: a locked or missing directory is not an error worth surfacing.
            let _ = std::fs::remove_dir_all(std::env::temp_dir().join("samplify-drag"));

            tray::setup(app.handle())?;
            hotkeys::register(app.handle());
            commands::start_poller(app.handle().clone());
            bridge::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            playground::save_playground_take,
            commands::list_marks, commands::search_marks, commands::get_tags,
            commands::save_client_id, commands::get_settings,
            commands::save_settings, commands::begin_auth,
            commands::get_library, commands::get_track_detail, commands::get_waveform,
            commands::get_marks_by_ids, commands::create_mark_from_selection,
            commands::update_mark_meta, commands::delete_mark, commands::set_hotkeys,
            commands::get_storage_info, commands::seed_dev_data, commands::popup_ready,
            commands::trigger_capture_cmd, commands::save_capture_cmd,
            commands::discard_capture_cmd, commands::set_favorite, commands::delete_track,
            commands::export_mark, commands::prepare_drag_wav, commands::update_mark_trim,
            commands::separate_mark_stems, commands::cancel_stem_separation, commands::stem_marks_for,
            commands::get_stem_set, commands::stem_model_status, commands::export_stem_mix,
            commands::prepare_stem_mix_drag, commands::delete_stem_set,
            commands::setup_spotify_integration, commands::spotify_install_kind,
            commands::get_tracks_by_tag,
            commands::list_collections, commands::create_collection, commands::delete_collection,
            commands::rename_collection, commands::set_mark_in_collection, commands::collections_for_mark,
            commands::get_collection_clips, commands::prepare_collection_export,
            commands::check_capture_status, commands::open_screen_recording_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Samplify");
}

mod tray {
    use tauri::Manager;
    use tauri::{
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
    };

    pub fn setup(app: &tauri::AppHandle) -> Result<(), tauri::Error> {
        let show = MenuItem::with_id(app, "show", "Show Library", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&show, &quit])?;

        let _ = TrayIconBuilder::new()
            .icon(app.default_window_icon().unwrap().clone())
            .menu(&menu)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "show" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            })
            .build(app)?;

        Ok(())
    }
}
