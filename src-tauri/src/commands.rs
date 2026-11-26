use crate::state::AppState;
use crate::{spotify, store};
use tauri::{Emitter, Manager, State};
use std::sync::{Mutex, OnceLock};

fn now_ms() -> i64 { chrono::Utc::now().timestamp_millis() }

pub struct FrozenSnapshot {
    pub resume_spotify: bool,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub trigger_now_ms: i64,
    pub trigger_track_ms: i64,
    pub spotify_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_art_url: Option<String>,
    pub duration_ms: i64,
}

static ACTIVE_CAPTURE: OnceLock<Mutex<Option<FrozenSnapshot>>> = OnceLock::new();

impl FrozenSnapshot {
    fn from_audio(
        samples: Vec<f32>, sample_rate: u32, channels: u16, trigger_now_ms: i64,
        track: Option<spotify::NowPlaying>, position: Option<(String, i64)>,
    ) -> Result<Self, String> {
        if sample_rate == 0 || channels == 0 || samples.is_empty() {
            return Err("Play audio through your default output, then press Capture after the phrase.".into());
        }
        if let Some(track) = track {
            let track_ms = position.filter(|(id, _)| id == &track.spotify_id)
                .map(|(_, ms)| ms).unwrap_or(track.progress_ms);
            // The buffer may still hold the previous song. Allow half a second
            // for metadata latency, preserving the original one-second floor.
            let samples = if track_ms > 0 {
                crate::capture::RingBuffer::trim_to_recent_ms(samples, sample_rate, channels, (track_ms + 500).max(1000))
            } else {
                samples
            };
            return Ok(Self {
                resume_spotify: track.is_playing,
                samples, sample_rate, channels, trigger_now_ms, trigger_track_ms: track_ms,
                spotify_id: track.spotify_id, title: track.title, artist: track.artist,
                album: track.album, album_art_url: track.album_art_url, duration_ms: track.duration_ms,
            });
        }

        // With no song clock the capture card's timeline starts at the first
        // frame of this snapshot. Each trigger is its own library entry.
        let duration_ms = (samples.len() / channels as usize) as i64 * 1000 / i64::from(sample_rate);
        let date = chrono::DateTime::from_timestamp_millis(trigger_now_ms)
            .ok_or("Invalid capture timestamp")?.with_timezone(&chrono::Local);
        Ok(Self {
            resume_spotify: false,
            samples, sample_rate, channels, trigger_now_ms, trigger_track_ms: duration_ms,
            spotify_id: format!("local:{}", uuid::Uuid::new_v4()),
            title: format!("Capture {}", date.format("%Y-%m-%d %H.%M.%S%.3f")),
            artist: "System audio".into(), album: String::new(), album_art_url: None, duration_ms,
        })
    }
}

#[cfg(test)]
mod capture_source_tests {
    use super::*;

    #[test]
    fn local_captures_have_independent_library_rows_and_buffer_relative_time() {
        let db = store::Store::open_in_memory().unwrap();
        let legacy = store::upsert_track(&db, "unknown", "Unknown Track", "Unknown Artist", "", None, 0).unwrap();
        let mut saved = Vec::new();
        for now in [1_753_200_001_100, 1_753_200_001_900] {
            let snap = FrozenSnapshot::from_audio(vec![0.1; 12_000], 1000, 2, now, None, Some(("old-song".into(), 500))).unwrap();
            assert_eq!(snap.samples.len(), 12_000, "a stale Spotify clock must not truncate local audio");
            assert_eq!(snap.trigger_track_ms, 6000, "the start of the buffer must display as zero");
            assert_eq!(snap.artist, "System audio");
            let id = store::upsert_track(&db, &snap.spotify_id, &snap.title, &snap.artist, "", None, snap.duration_ms).unwrap();
            assert_ne!(id, legacy);
            saved.push((id, snap.title));
        }
        assert_ne!(saved[0].0, saved[1].0);
        assert_ne!(saved[0].1, saved[1].1);
    }

    #[test]
    fn identified_capture_keeps_the_song_clock_and_track_boundary() {
        let track = spotify::NowPlaying {
            is_track: true, spotify_id: "known".into(), title: "Song".into(), artist: "Artist".into(),
            album: "Album".into(), album_art_url: None, duration_ms: 9000, progress_ms: 1500, is_playing: true,
        };
        let snap = FrozenSnapshot::from_audio(vec![0.1; 12_000], 1000, 2, 7000, Some(track), Some(("known".into(), 2000))).unwrap();
        assert_eq!(snap.trigger_track_ms, 2000);
        assert_eq!(snap.samples.len(), 5000);
        assert_eq!((snap.spotify_id.as_str(), snap.title.as_str()), ("known", "Song"));
    }
}

fn get_active_capture() -> &'static Mutex<Option<FrozenSnapshot>> {
    ACTIVE_CAPTURE.get_or_init(|| Mutex::new(None))
}

fn calculate_peaks_f32(samples: &[f32], channels: u16, buckets: u32) -> Vec<f32> {
    let ch = channels as usize;
    if ch == 0 || samples.is_empty() || buckets == 0 { return vec![0.0; buckets as usize]; }
    let total_frames = samples.len() / ch;
    if total_frames == 0 { return vec![0.0; buckets as usize]; }
    
    let mut peaks = vec![0.0f32; buckets as usize];
    for f in 0..total_frames {
        let bucket_idx = ((f as u64 * buckets as u64) / total_frames as u64) as usize;
        let bucket_idx = bucket_idx.min(buckets as usize - 1);
        
        let mut frame_max = 0.0f32;
        for c in 0..ch {
            let idx = f * ch + c;
            if idx < samples.len() {
                let val = samples[idx].abs();
                if val > frame_max { frame_max = val; }
            }
        }
        if frame_max > peaks[bucket_idx] {
            peaks[bucket_idx] = frame_max;
        }
    }
    peaks
}

#[tauri::command]
pub async fn trigger_capture_cmd(app: tauri::AppHandle) -> Result<(), String> {
    trigger_capture(app, false).await
}

pub async fn trigger_capture(app: tauri::AppHandle, from_playbar: bool) -> Result<(), String> {
    let state = app.state::<AppState>();

    // 0. macOS gate: if the Screen Recording TCC permission hasn't been
    //    granted (or was denied), the ring buffer is empty and we'd otherwise
    //    return the misleading "No audio captured" message. Surface the real
    //    reason so the user knows to grant + restart rather than fight Spotify.
    match crate::capture::authorization() {
        crate::capture::CaptureAuth::Authorized => {}
        crate::capture::CaptureAuth::NotDetermined => return Err(
            "Samplify needs Screen Recording permission to capture audio. Grant it in System Settings \u{2192} Privacy & Security \u{2192} Screen Recording, then restart Samplify.".into(),
        ),
        crate::capture::CaptureAuth::Denied => return Err(
            "Screen Recording permission was denied. Open System Settings \u{2192} Privacy & Security \u{2192} Screen Recording, enable Samplify, then restart.".into(),
        ),
        crate::capture::CaptureAuth::Restricted => return Err(
            "Capture is restricted by a configuration profile (MDM) on this Mac.".into(),
        ),
    }

    // 1. Track metadata: prefer what the Spicetify extension pushed (it's exactly
    //    the client the user is capturing from); fall back to the Web API.
    let system = cfg!(windows) && !from_playbar && state.config.lock().unwrap().system_capture;
    let bridge_track = state.spotify_player_track();
    let current = if system {
        None
    } else if bridge_track.is_some() || from_playbar {
        bridge_track
    } else {
        let token = state.tokens.lock().unwrap().clone();
        match token {
            Some(t) => spotify::fetch_now_playing(&t.access_token).await.ok().flatten(),
            None => None,
        }
    };
    let trigger_now_ms = now_ms();
    #[cfg(windows)]
    let (samples, sr, ch, position) = if system {
        let rb = state.ring.lock().unwrap();
        (rb.fresh_system_snapshot(), rb.sample_rate, rb.channels, None)
    } else {
        let track = current.as_ref().ok_or("Open a track in Spotify on this PC. Enable the playbar integration in Settings, or connect the Web API.")?;
        let audio = state.desktop_audio.spotify.lock().unwrap();
        let (samples, end) = audio.freeze(&track.spotify_id, std::time::Instant::now())?;
        (samples, crate::capture_win::RATE, crate::capture_win::CHANNELS, Some((track.spotify_id.clone(), end)))
    };
    #[cfg(target_os = "macos")]
    let (samples, sr, ch, position) = {
        let rb = state.ring.lock().unwrap();
        (rb.snapshot(), rb.sample_rate, rb.channels, state.clock.lock().unwrap().position(trigger_now_ms))
    };
    let snap = FrozenSnapshot::from_audio(samples, sr, ch, trigger_now_ms, current, position)?;
    // 6. Write the temporary WAV file to Clips
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let temp_wav_path = clips_dir.join(".temp_capture.wav");
    crate::session::write_wav_i16(&temp_wav_path, &snap.samples, sr, ch)
        .map_err(|e| format!("Failed to write temporary WAV: {}", e))?;

    if snap.resume_spotify {
        set_spotify_playback(&state, false).await;
    }

    // 7. Launch popup
    let title = snap.title.clone();
    let trigger_track_ms = snap.trigger_track_ms;
    *get_active_capture().lock().unwrap() = Some(snap);
    crate::popup::show_popup(&app, 0, &title, trigger_track_ms);

    Ok(())
}

#[tauri::command]
pub async fn save_capture_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    note: String,
    tags: Vec<String>,
    start_ms: f64,
    end_ms: f64,
) -> Result<(), String> {
    let snap = {
        let mut active = get_active_capture().lock().unwrap();
        active.take().ok_or_else(|| "No active capture to save".to_string())?
    };

    let sr = snap.sample_rate;
    let ch = snap.channels as usize;
    
    // 1. Slice selected region (frame-aligned)
    let start_idx = ((start_ms / 1000.0) * sr as f64).round() as usize * ch;
    let end_idx = ((end_ms / 1000.0) * sr as f64).round() as usize * ch;
    
    let slice = snap.samples[start_idx.min(snap.samples.len())..end_idx.clamp(start_idx, snap.samples.len())].to_vec();
    if slice.is_empty() {
        return Err("Selected slice is empty".to_string());
    }

    // 2. Generate file name
    let trigger_dt = chrono::DateTime::from_timestamp(snap.trigger_now_ms / 1000, ((snap.trigger_now_ms % 1000) * 1_000_000) as u32)
        .unwrap_or_else(|| chrono::Utc::now().into());
    let local_dt: chrono::DateTime<chrono::Local> = trigger_dt.into();
    let ts = local_dt.format("%Y-%m-%d %H%M%S").to_string();
    let filename = crate::session::sanitize_filename(&snap.artist, &snap.title, &ts);

    // 3. Write final WAV file
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let file_path = clips_dir.join(&filename);
    crate::session::write_wav_i16(&file_path, &slice, sr, snap.channels)
        .map_err(|e| format!("Failed to write final WAV: {}", e))?;

    // 4. Calculate file_start_track_ms
    let snapshot_duration_ms = ((snap.samples.len() as f64 / ch as f64 / sr as f64) * 1000.0) as i64;
    let file_start_track_ms = (snap.trigger_track_ms - (snapshot_duration_ms - start_ms as i64)).max(0);

    // 5. Database operations
    let track_id = store::upsert_track(
        &state.store,
        &snap.spotify_id,
        &snap.title,
        &snap.artist,
        &snap.album,
        snap.album_art_url.as_deref(),
        snap.duration_ms,
    ).map_err(|e| e.to_string())?;

    let file_path_str = file_path.to_string_lossy().to_string();
    let session_id = store::insert_session(
        &state.store,
        track_id,
        &file_path_str,
        file_start_track_ms,
        sr as i64,
        snap.channels as i64,
    ).map_err(|e| e.to_string())?;

    let mark_id = store::insert_mark(
        &state.store,
        track_id,
        file_start_track_ms,
        Some(session_id),
        Some(0),
        &note,
    ).map_err(|e| e.to_string())?;

    if !tags.is_empty() {
        store::set_mark_text_tags(&state.store, mark_id, &note, &tags).map_err(|e| e.to_string())?;
    }

    // 6. Emit creations to refresh library view
    let _ = app.emit("mark-created", serde_json::json!({ "mark_id": mark_id, "want_popup": false }));

    // 7. Best-effort temp cleanup (popup released its audio handle before invoking).
    let _ = std::fs::remove_file(clips_dir.join(".temp_capture.wav"));

    // 8. Resume Spotify + tell the extension to refresh its seekbar markers so the
    //    new note appears immediately.
    crate::bridge::broadcast(serde_json::json!({ "type": "refresh_marks" }));
    if snap.resume_spotify { set_spotify_playback(&state, true).await; }

    Ok(())
}

/// Drops the frozen snapshot (frees ~45MB at default settings) and removes the
/// temp WAV. Called by the capture card on every discard path (Esc/✕/focus loss).
#[tauri::command]
pub async fn discard_capture_cmd(state: State<'_, AppState>) -> Result<(), String> {
    let snap = get_active_capture().lock().unwrap().take();
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let _ = std::fs::remove_file(clips_dir.join(".temp_capture.wav"));
    if snap.is_some_and(|s| s.resume_spotify) { set_spotify_playback(&state, true).await; }
    Ok(())
}

async fn set_spotify_playback(state: &AppState, play: bool) {
    if state.spotify_player_track().is_some() {
        crate::bridge::broadcast(serde_json::json!({ "type": if play { "resume" } else { "pause" } }));
    } else {
        let token = state.tokens.lock().unwrap().clone();
        if let Some(t) = token {
            if play { let _ = spotify::resume(&t.access_token).await; }
            else { let _ = spotify::pause(&t.access_token).await; }
        }
    }
}

#[tauri::command]
pub fn popup_ready(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let active = get_active_capture().lock().unwrap();
    if let Some(ref snap) = *active {
        // 2000 buckets over the snapshot gives fine-grained detail when the
        // capture card zooms into a few seconds of audio.
        let peaks = calculate_peaks_f32(&snap.samples, snap.channels, 2000);
        let snapshot_duration_ms = ((snap.samples.len() as f64 / snap.channels as f64 / snap.sample_rate as f64) * 1000.0) as i64;
        
        let clips_dir = app.state::<AppState>().config.lock().unwrap().clips_dir.clone();
        let temp_wav_path = clips_dir.join(".temp_capture.wav");
        
        Ok(serde_json::json!({
            "title": snap.title,
            "artist": snap.artist,
            "trigger_track_ms": snap.trigger_track_ms,
            "snapshot_duration_ms": snapshot_duration_ms,
            "temp_wav_path": temp_wav_path.to_string_lossy(),
            "waveform": peaks,
            "album_art_url": snap.album_art_url,
        }))
    } else {
        Err("No active capture".to_string())
    }
}

#[tauri::command]
pub fn list_marks(state: State<AppState>) -> Result<Vec<store::MarkRow>, String> {
    store::list_marks(&state.store).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn search_marks(state: State<AppState>, query: String) -> Result<Vec<store::MarkRow>, String> {
    if query.trim().is_empty() { return store::list_marks(&state.store).map_err(|e| e.to_string()); }
    store::search_marks(&state.store, &query).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_tags(state: State<AppState>) -> Result<Vec<String>, String> {
    store::tag_names(&state.store).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_client_id(state: State<AppState>, client_id: String) -> Result<(), String> {
    store::set_setting(&state.store, "client_id", &client_id).map_err(|e| e.to_string())?;
    state.config.lock().unwrap().client_id = client_id;
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Result<serde_json::Value, String> {
    let c = state.config.lock().unwrap();
    Ok(serde_json::json!({
        "pre_roll": c.pre_roll, "post_roll": c.post_roll,
        "client_id": c.client_id, "clips_dir": c.clips_dir.to_string_lossy(),
        "hotkey_silent": "".to_string(), "hotkey_popup": c.capture_hotkey,
        "capture_hotkey": c.capture_hotkey,
        "buffer_seconds": c.buffer_seconds,
        "source_selection": cfg!(windows), "system_capture": c.system_capture,
        "spotify_player_active": state.spotify_player_track().is_some(),
        "authenticated": state.tokens.lock().unwrap().is_some(),
        "debug": cfg!(debug_assertions),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[tauri::command]
pub fn save_settings(state: State<AppState>, buffer_seconds: f64, system_capture: Option<bool>) -> Result<(), String> {
    if !(30.0..=300.0).contains(&buffer_seconds) { return Err("Choose a buffer between 30 and 300 seconds.".into()); }
    store::set_setting(&state.store, "buffer_seconds", &buffer_seconds.to_string()).map_err(|e| e.to_string())?;
    let mut c = state.config.lock().unwrap();
    c.buffer_seconds = buffer_seconds;

    let mut ring = state.ring.lock().unwrap();
    ring.set_capacity_seconds(buffer_seconds);
    #[cfg(windows)]
    {
        state.desktop_audio.spotify.lock().unwrap().set_capacity_seconds(buffer_seconds);
        if let Some(enabled) = system_capture.filter(|&enabled| enabled != c.system_capture) {
            store::set_setting(&state.store, "capture_source", if enabled { "system" } else { "spotify" }).map_err(|e| e.to_string())?;
            c.system_capture = enabled;
            state.desktop_audio.system_enabled.store(enabled, std::sync::atomic::Ordering::Release);
            ring.clear();
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn begin_auth(app: tauri::AppHandle) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let state = app.state::<AppState>();
    let client_id = state.config.lock().unwrap().client_id.clone();
    if client_id.is_empty() { return Err("Set your Spotify Client ID first".into()); }

    // Only one login at a time. A second click while the first is still waiting for
    // the browser redirect would open a new auth URL with a fresh CSRF state, but the
    // first login still owns the loopback port — so it would catch the second
    // redirect and report a false "state mismatch". Reject the duplicate up-front.
    if state.auth_in_progress.swap(true, Ordering::SeqCst) {
        return Err("A Spotify login is already in progress — finish it in the browser tab that \
                    opened, or wait a few seconds and try again.".into());
    }
    // Release the flag however we leave this function (success, error, timeout).
    struct Guard<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for Guard<'_> {
        fn drop(&mut self) { self.0.store(false, std::sync::atomic::Ordering::SeqCst); }
    }
    let _guard = Guard(&state.auth_in_progress);

    let verifier = spotify::generate_verifier();
    let challenge = spotify::challenge_for(&verifier);
    let csrf = spotify::generate_verifier(); // reuse as random state
    let url = spotify::authorize_url(&client_id, spotify::REDIRECT_URI, &challenge, &csrf, spotify::SCOPES);

    // Open the browser (uses the OS default handler).
    let _ = crate::platform::open_url(&url);

    // Run the blocking listener off the async runtime, with a timeout so an abandoned
    // login releases the loopback port instead of holding it until the app restarts.
    let (code, got_state) = tokio::task::spawn_blocking(|| {
        spotify::await_callback(5723, std::time::Duration::from_secs(180))
    }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    if got_state != csrf {
        return Err("Login could not be verified (state mismatch). Please click Connect once and \
                    complete a single login in the browser tab that opens.".into());
    }

    let tokens = spotify::exchange_code(&client_id, &code, &verifier, spotify::REDIRECT_URI)
        .await.map_err(|e| e.to_string())?;
    spotify::save_tokens(&state.store, &tokens);
    *state.tokens.lock().unwrap() = Some(tokens);
    let _ = app.emit("auth-changed", serde_json::json!({"authenticated": true}));
    Ok(())
}

pub fn start_poller(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            let state = app.state::<AppState>();
            let now = now_ms();

            // Refresh token if within 60s of expiry.
            let (access, client_id) = {
                let tok = state.tokens.lock().unwrap().clone();
                let cid = state.config.lock().unwrap().client_id.clone();
                (tok, cid)
            };
            if let Some(mut t) = access {
                if t.expires_at_unix - chrono::Utc::now().timestamp() < 60 {
                    if let Ok(nt) = spotify::refresh(&client_id, &t.refresh_token).await {
                        spotify::save_tokens(&state.store, &nt);
                        *state.tokens.lock().unwrap() = Some(nt.clone());
                        t = nt;
                    }
                }

                // Poll Spotify Now Playing.
                if let Ok(Some(np)) = spotify::fetch_now_playing(&t.access_token).await {
                    if state.spotify_player_track().is_none() {
                        #[cfg(windows)]
                        state.desktop_audio.spotify.lock().unwrap().update(&np.spotify_id, np.progress_ms, np.is_playing, std::time::Instant::now());
                        let mut clk = state.clock.lock().unwrap();
                        clk.update(crate::clock::Sample {
                            track_ms: np.progress_ms, is_playing: np.is_playing,
                            received_at_ms: now, spotify_id: np.spotify_id.clone(),
                        });
                    }
                    let _ = app.emit("now-playing", serde_json::json!({
                        "title": np.title, "artist": np.artist, "album_art_url": np.album_art_url,
                        "progress_ms": np.progress_ms, "duration_ms": np.duration_ms,
                        "is_playing": np.is_playing, "is_track": np.is_track,
                    }));
                }
            }
        }
    });
}

// ---- Phase 1.5 New Commands ----

#[tauri::command]
pub fn get_library(state: State<AppState>) -> Result<Vec<store::LibraryTrack>, String> {
    store::get_library(&state.store).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_tracks_by_tag(state: State<AppState>, tag: String) -> Result<Vec<store::LibraryTrack>, String> {
    store::tracks_by_tag(&state.store, &tag).map_err(|e| e.to_string())
}

// ---- Collections ----

#[tauri::command]
pub fn list_collections(state: State<AppState>) -> Result<Vec<store::Collection>, String> {
    store::list_collections(&state.store).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_collection(state: State<AppState>, name: String) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() { return Err("Collection name can't be empty".into()); }
    store::create_collection(&state.store, name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_collection(state: State<AppState>, id: i64) -> Result<(), String> {
    store::delete_collection(&state.store, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_collection(state: State<AppState>, id: i64, name: String) -> Result<(), String> {
    store::rename_collection(&state.store, id, name.trim()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_mark_in_collection(state: State<AppState>, collection_id: i64, mark_id: i64, member: bool) -> Result<(), String> {
    store::set_mark_in_collection(&state.store, collection_id, mark_id, member).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn collections_for_mark(state: State<AppState>, mark_id: i64) -> Result<Vec<i64>, String> {
    store::collections_for_mark(&state.store, mark_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_collection_clips(state: State<AppState>, collection_id: i64) -> Result<Vec<store::CollectionClip>, String> {
    store::collection_clips(&state.store, collection_id).map_err(|e| e.to_string())
}

/// Materialize a collection as a folder of (trimmed) WAV files and return their
/// paths, so the frontend can drag the whole pack into a DAW in one gesture.
#[tauri::command]
pub fn prepare_collection_export(state: State<AppState>, collection_id: i64) -> Result<Vec<String>, String> {
    let clips = store::collection_clips(&state.store, collection_id).map_err(|e| e.to_string())?;
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let pack_dir = clips_dir.parent().unwrap_or(&clips_dir).join("Packs").join(format!("collection_{collection_id}"));
    // Rebuild the pack fresh so it reflects the current collection.
    let _ = std::fs::remove_dir_all(&pack_dir);
    std::fs::create_dir_all(&pack_dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for (i, clip) in clips.iter().enumerate() {
        let fname = crate::session::sanitize_filename(&clip.artist, &clip.title, &format!("{:02}", i + 1));
        let dest = pack_dir.join(&fname);
        let src = std::path::Path::new(&clip.file_path);
        let ok = match (clip.trim_start_ms, clip.trim_end_ms) {
            (Some(ts), Some(te)) if te > ts => crate::session::write_trimmed_wav(src, &dest, ts, te).is_ok(),
            _ => std::fs::copy(src, &dest).is_ok(),
        };
        if ok { paths.push(dest.to_string_lossy().to_string()); }
    }
    if paths.is_empty() { return Err("This collection has no clips to drag".into()); }
    Ok(paths)
}

#[tauri::command]
pub fn get_track_detail(state: State<AppState>, track_id: i64) -> Result<store::TrackDetail, String> {
    store::get_track_detail(&state.store, track_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_marks_by_ids(state: State<AppState>, ids: Vec<i64>) -> Result<Vec<store::PlaygroundClip>, String> {
    store::get_marks_by_ids(&state.store, &ids).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_waveform(state: State<AppState>, session_id: i64, buckets: u32) -> Result<Vec<f32>, String> {
    let (file_path, channels) = store::get_session_info(&state.store, session_id).map_err(|e| e.to_string())?;
    store::calculate_waveform(&file_path, channels, buckets).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_mark_meta(
    state: State<AppState>,
    mark_id: i64,
    note_text: String,
    tags: Vec<String>,
    rating: Option<i64>,
    key: Option<String>,
    bpm: Option<i64>,
) -> Result<(), String> {
    store::update_mark_meta(&state.store, mark_id, &note_text, &tags, rating, key, bpm).map_err(|e| e.to_string())?;
    // Note text/tags feed the Spotify seekbar tooltips — refresh them live.
    crate::bridge::broadcast(serde_json::json!({ "type": "refresh_marks" }));
    Ok(())
}

#[tauri::command]
pub fn delete_mark(state: State<AppState>, mark_id: i64) -> Result<(), String> {
    store::delete_mark(&state.store, mark_id).map_err(|e| e.to_string())?;
    // Remove the marker from Spotify's seekbar immediately.
    crate::bridge::broadcast(serde_json::json!({ "type": "refresh_marks" }));
    Ok(())
}

#[tauri::command]
pub fn set_favorite(state: State<AppState>, track_id: i64, favorite: bool) -> Result<(), String> {
    store::set_favorite(&state.store, track_id, favorite).map_err(|e| e.to_string())
}

/// Deletes an entire track (marks, sessions, and the clip WAVs on disk). Used to
/// clean up whole tracks — including pre-v2 captures with misplaced pins.
#[tauri::command]
pub fn delete_track(state: State<AppState>, track_id: i64) -> Result<(), String> {
    let files = store::delete_track(&state.store, track_id).map_err(|e| e.to_string())?;
    for f in files {
        let _ = std::fs::remove_file(&f);
    }
    crate::bridge::broadcast(serde_json::json!({ "type": "refresh_marks" }));
    Ok(())
}

/// Exports a mark's clip to a sibling `Exports/` folder with a DAW-friendly name
/// and reveals it in Explorer. The v2 clip is already the drag-selected region,
/// so this is a straight copy. Returns the exported file path.
#[tauri::command]
pub fn export_mark(state: State<AppState>, mark_id: i64) -> Result<String, String> {
    let info = store::get_mark_for_export(&state.store, mark_id).map_err(|e| e.to_string())?;
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let export_dir = clips_dir.parent().unwrap_or(&clips_dir).join("Exports");
    std::fs::create_dir_all(&export_dir).map_err(|e| e.to_string())?;

    let min = info.track_ms / 60000;
    let sec = (info.track_ms % 60000) / 1000;
    let tag_suffix = if info.tags.is_empty() { String::new() } else { format!(" ({})", info.tags.join(",")) };
    let label = format!("{}.{:02}{}", min, sec, tag_suffix);
    let fname = crate::session::sanitize_filename(&info.artist, &info.title, &label);
    let dest = export_dir.join(&fname);

    // Export the trimmed region if one is set, else the whole clip.
    match (info.trim_start_ms, info.trim_end_ms) {
        (Some(ts), Some(te)) if te > ts => {
            crate::session::write_trimmed_wav(std::path::Path::new(&info.file_path), &dest, ts, te)
                .map_err(|e| format!("trim export failed: {e}"))?;
        }
        _ => {
            std::fs::copy(&info.file_path, &dest).map_err(|e| format!("copy failed: {e}"))?;
        }
    }

    // Best-effort: reveal the exported file in the OS file manager (File
    // Explorer on Windows with `/select,"PATH"`; Finder on macOS with -R),
    // selecting the file. The Windows `/select` quoting gotcha is handled
    // inside `platform::reveal_path` (raw_arg emits the exact `/select,"PATH"`
    // form Explorer parses correctly).
    let _ = crate::platform::reveal_path(&dest);
    Ok(dest.to_string_lossy().to_string())
}

/// Split a note into four stems (drums / bass / other / vocals).
///
/// Exactly one runs at a time app-wide — one inference peaks near 4.9 GB, so two
/// would mean ~10 GB. The source capture is only ever read; stems are new files,
/// and nothing is recorded in the library unless all four were written.
#[tauri::command]
pub async fn separate_mark_stems(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mark_id: i64,
) -> Result<Vec<i64>, String> {
    let info = store::get_mark_for_export(&state.store, mark_id).map_err(|e| e.to_string())?;
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let model = crate::stems::separator::resolve_model_path(&app).map_err(|e| e.to_string())?;
    if !model.exists() {
        return Err(format!(
            "Stem separation model not found at {}. Run `npm run fetch-model`.",
            model.display()
        ));
    }

    let trim = match (info.trim_start_ms, info.trim_end_ms) {
        (Some(ts), Some(te)) if te > ts => Some((ts, te)),
        _ => None,
    };

    // Refuse over-long clips up front, naming the cap.
    let region_ms = match trim {
        Some((ts, te)) => te - ts,
        None => {
            let r = hound::WavReader::open(&info.file_path).map_err(|e| e.to_string())?;
            let spec = r.spec();
            ((r.duration() as f64 / spec.sample_rate as f64) * 1000.0) as i64
        }
    };
    let cap = crate::stems::job::MAX_CLIP_MS;
    if region_ms > cap {
        return Err(format!(
            "That clip is {:.0}s — stem separation is capped at {}s. Trim it tighter first.",
            region_ms as f64 / 1000.0,
            cap / 1000
        ));
    }

    let guard = crate::stems::job::JobGuard::acquire()
        .ok_or_else(|| "A stem split is already running.".to_string())?;

    let src_path = info.file_path.clone();
    let artist = info.artist.clone();
    let title = info.title.clone();
    let app_for_job = app.clone();

    // Audio work off the main thread; the Store stays here because it is not Send.
    let baked = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<Vec<std::path::PathBuf>> {
        let _guard = guard; // released when this closure ends, however it ends

        // Separate exactly the region the note plays. A trimmed note is sliced to a
        // temp file first so the model never sees audio outside the loop.
        let tmp_dir = std::env::temp_dir().join("samplify-stems");
        let source = match trim {
            Some((ts, te)) => {
                std::fs::create_dir_all(&tmp_dir)?;
                let t = tmp_dir.join(format!("src-{mark_id}-{ts}-{te}.wav"));
                crate::session::write_trimmed_wav(std::path::Path::new(&src_path), &t, ts, te)?;
                t
            }
            None => std::path::PathBuf::from(&src_path),
        };

        let mut sep = crate::stems::separator::OrtSeparator::new(&model)?;
        let mut on_progress = |done: usize, total: usize| {
            let _ = app_for_job.emit(
                "stem-progress",
                serde_json::json!({ "markId": mark_id, "done": done, "total": total }),
            );
            !crate::stems::job::is_cancelled()
        };
        let out = crate::stems::separate_to_files(
            &mut sep, &source, &clips_dir, &artist, &title, &mut on_progress,
        );
        // Whatever happened, don't leave the sliced source behind.
        if trim.is_some() {
            let _ = std::fs::remove_file(&source);
        }
        match out {
            Ok(paths) => Ok(paths),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| format!("stem job failed to run: {e}"))?;

    let paths = match baked {
        Ok(p) => p,
        Err(e) => return Err(e.to_string()),
    };

    // Record them. If this fails the stems are orphaned on disk, so clean up.
    let stems: Vec<store::StemInsert> = paths
        .iter()
        .zip(crate::stems::separator::STEM_NAMES)
        .map(|(p, name)| {
            // Read the real spec back rather than assuming: the stems inherit the
            // capture's rate, and a wrong value here would mis-time playback.
            let (sr, ch) = hound::WavReader::open(p)
                .map(|r| (r.spec().sample_rate as i64, r.spec().channels as i64))
                .unwrap_or((44_100, 2));
            store::StemInsert {
                file_path: p.to_string_lossy().to_string(),
                stem: name.to_string(),
                sample_rate: sr,
                channels: ch,
            }
        })
        .collect();

    match store::insert_stem_marks(&state.store, mark_id, &stems) {
        Ok(ids) => Ok(ids),
        Err(e) => {
            for p in &paths {
                let _ = std::fs::remove_file(p);
            }
            Err(format!("stems were separated but could not be saved: {e}"))
        }
    }
}

/// Ask a running split to stop after the current chunk.
#[tauri::command]
pub fn cancel_stem_separation() -> Result<(), String> {
    crate::stems::job::request_cancel();
    Ok(())
}

/// Mark ids already produced by splitting this mark, so the UI can show "4 stems"
/// rather than offering to split again.
#[tauri::command]
pub fn stem_marks_for(state: State<AppState>, mark_id: i64) -> Result<Vec<i64>, String> {
    store::stem_marks_for(&state.store, mark_id).map_err(|e| e.to_string())
}

/// The four stems split from a note (or the set a stem belongs to), for the mixer.
/// `None` when the note has never been split.
#[tauri::command]
pub fn get_stem_set(state: State<AppState>, mark_id: i64) -> Result<Option<store::StemSet>, String> {
    store::get_stem_set(&state.store, mark_id).map_err(|e| e.to_string())
}

/// Whether the bundled separation model is where the app expects it, so the UI
/// can disable splitting with a reason instead of failing on click.
#[tauri::command]
pub fn stem_model_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let path = crate::stems::separator::resolve_model_path(&app).map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(serde_json::json!({
        "present": path.exists() && bytes > 0,
        "path": path.to_string_lossy(),
        "bytes": bytes,
        "max_clip_seconds": crate::stems::job::MAX_CLIP_MS / 1000,
    }))
}

/// Mix a stem set at the mixer's fader positions into `dest`. Shared by export,
/// save-as-note and drag-out so all three bounce byte-identical audio.
fn bounce_stem_mix(
    state: &AppState,
    source_mark_id: i64,
    gains: &std::collections::HashMap<String, f32>,
    dest: &std::path::Path,
) -> Result<(store::StemSet, Vec<String>, u32, u16), String> {
    let set = store::get_stem_set(&state.store, source_mark_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "This note has no stems yet — split it first.".to_string())?;
    let gain_for = |stem: &str| gains.get(stem).copied().unwrap_or(1.0).clamp(0.0, 2.0);
    let included: Vec<String> = set.stems.iter()
        .filter(|s| gain_for(&s.stem) > 0.0)
        .map(|s| s.stem.clone())
        .collect();
    let paths: Vec<std::path::PathBuf> = set.stems.iter().map(|s| std::path::PathBuf::from(&s.file_path)).collect();
    let inputs: Vec<crate::stems::mix::MixInput<'_>> = set.stems.iter().zip(paths.iter())
        .map(|(s, p)| crate::stems::mix::MixInput { path: p, gain: gain_for(&s.stem) })
        .collect();
    let (sr, ch) = crate::stems::mix::mix_to_wav(&inputs, dest).map_err(|e| e.to_string())?;
    Ok((set, included, sr, ch))
}

fn mix_label(included: &[String]) -> String {
    if included.len() == store::RAW_STEMS.len() { "mix".to_string() }
    else { format!("mix {}", included.join("+")) }
}

/// Bounce the mixer's current balance to a WAV. With `save_as_note` the mix lands
/// in the library as an ordinary note on the same track (so it can be trimmed,
/// tagged, dragged and played like any capture); otherwise it goes to `Exports/`
/// and is revealed in the file manager. Returns `{ path, mark_id? }`.
#[tauri::command]
pub fn export_stem_mix(
    state: State<AppState>,
    source_mark_id: i64,
    gains: std::collections::HashMap<String, f32>,
    save_as_note: bool,
) -> Result<serde_json::Value, String> {
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let set = store::get_stem_set(&state.store, source_mark_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "This note has no stems yet — split it first.".to_string())?;
    let included: Vec<String> = set.stems.iter()
        .filter(|s| gains.get(&s.stem).copied().unwrap_or(1.0) > 0.0)
        .map(|s| s.stem.clone())
        .collect();
    let min = set.track_ms / 60000;
    let sec = (set.track_ms % 60000) / 1000;

    if save_as_note {
        // Stamped so bouncing twice never overwrites the first take.
        let stamp = chrono::Local::now().format("%H%M%S");
        let fname = crate::session::sanitize_filename(
            &set.artist, &set.title, &format!("{}.{:02} {} {}", min, sec, mix_label(&included), stamp));
        let dest = clips_dir.join(fname);
        let (_, included, sr, ch) = bounce_stem_mix(&state, source_mark_id, &gains, &dest)?;
        let inc: Vec<&str> = included.iter().map(|s| s.as_str()).collect();
        let mark_id = match store::insert_mix_mark(&state.store, source_mark_id, &dest.to_string_lossy(), &inc, sr as i64, ch as i64) {
            Ok(id) => id,
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                return Err(format!("the mix was written but could not be saved as a note: {e}"));
            }
        };
        crate::bridge::broadcast(serde_json::json!({ "type": "refresh_marks" }));
        Ok(serde_json::json!({ "path": dest.to_string_lossy(), "mark_id": mark_id }))
    } else {
        let export_dir = clips_dir.parent().unwrap_or(&clips_dir).join("Exports");
        std::fs::create_dir_all(&export_dir).map_err(|e| e.to_string())?;
        let fname = crate::session::sanitize_filename(
            &set.artist, &set.title, &format!("{}.{:02} {}", min, sec, mix_label(&included)));
        let dest = export_dir.join(fname);
        bounce_stem_mix(&state, source_mark_id, &gains, &dest)?;
        let _ = crate::platform::reveal_path(&dest);
        Ok(serde_json::json!({ "path": dest.to_string_lossy() }))
    }
}

/// A temp WAV of the current mix for the OS drag, named by its fader positions so
/// dragging the same balance twice reuses the file and a new balance gets a new one.
#[tauri::command]
pub fn prepare_stem_mix_drag(
    state: State<AppState>,
    source_mark_id: i64,
    gains: std::collections::HashMap<String, f32>,
) -> Result<String, String> {
    let mut sig: Vec<String> = store::RAW_STEMS.iter()
        .map(|s| format!("{}{}", &s[..1], (gains.get(*s).copied().unwrap_or(1.0).clamp(0.0, 2.0) * 100.0).round() as i64))
        .collect();
    sig.dedup();
    let dir = std::env::temp_dir().join("samplify-drag");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("mix-{}-{}.wav", source_mark_id, sig.join("")));
    if !dest.exists() {
        bounce_stem_mix(&state, source_mark_id, &gains, &dest)?;
    }
    Ok(dest.to_string_lossy().to_string())
}

/// Delete the four stems split from a note — rows and WAVs. The source capture
/// and any mixes saved as notes are untouched.
#[tauri::command]
pub fn delete_stem_set(state: State<AppState>, source_mark_id: i64) -> Result<(), String> {
    let files = store::delete_stem_set(&state.store, source_mark_id).map_err(|e| e.to_string())?;
    for f in files {
        let _ = std::fs::remove_file(&f);
    }
    crate::bridge::broadcast(serde_json::json!({ "type": "refresh_marks" }));
    Ok(())
}

/// Path the note-row drag handle should hand to the OS. Mirrors export_mark's
/// trim semantics so dragging into a DAW and exporting produce the same audio.
#[tauri::command]
pub fn prepare_drag_wav(state: State<AppState>, mark_id: i64) -> Result<String, String> {
    let info = store::get_mark_for_export(&state.store, mark_id).map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join("samplify-drag");
    let trim = match (info.trim_start_ms, info.trim_end_ms) {
        (Some(ts), Some(te)) => Some((ts, te)),
        _ => None,
    };
    let path = crate::session::prepare_drag_slice(
        &dir, std::path::Path::new(&info.file_path), &info.artist, &info.title, mark_id, trim)
        .map_err(|e| format!("drag slice failed: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn update_mark_trim(state: State<AppState>, mark_id: i64, trim_start_ms: Option<i64>, trim_end_ms: Option<i64>) -> Result<(), String> {
    store::update_mark_trim(&state.store, mark_id, trim_start_ms, trim_end_ms).map_err(|e| e.to_string())
}

/// Save the library waveform's current selection as a brand-new note on the same
/// capture. Row insert only — the capture's WAV is untouched.
#[tauri::command]
pub fn create_mark_from_selection(
    state: State<AppState>,
    session_id: i64,
    trim_start_ms: i64,
    trim_end_ms: i64,
    note_text: String,
    tags: Vec<String>,
) -> Result<i64, String> {
    store::create_mark_from_selection(
        &state.store, session_id, trim_start_ms, trim_end_ms, &note_text, &tags)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_hotkeys(app: tauri::AppHandle, capture_hotkey: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    
    // Live re-registration of global shortcuts
    crate::hotkeys::re_register(&app, &capture_hotkey)?;
    
    let _ = store::set_setting(&state.store, "capture_hotkey", &capture_hotkey);
    
    {
        let mut c = state.config.lock().unwrap();
        c.capture_hotkey = capture_hotkey;
    }
    Ok(())
}

#[derive(serde::Serialize)]
pub struct StorageInfo {
    pub free_gb: f64,
    pub total_gb: f64,
}

#[tauri::command]
pub fn get_storage_info(state: State<AppState>) -> Result<StorageInfo, String> {
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let parent = clips_dir.parent().unwrap_or(&clips_dir);
    let _ = std::fs::create_dir_all(parent);
    
    let free_space = fs2::free_space(parent).unwrap_or(0);
    let total_space = fs2::total_space(parent).unwrap_or(0);
    
    let free_gb = free_space as f64 / 1024.0 / 1024.0 / 1024.0;
    let total_gb = total_space as f64 / 1024.0 / 1024.0 / 1024.0;
    
    Ok(StorageInfo { free_gb, total_gb })
}

/// Build a Command that doesn't flash a console window on Windows. Currently
/// a thin alias for `platform::silent_command`; kept for call-site clarity
/// (Spicetify install path) and to avoid a wide churn diff.
fn cmd_no_window(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    crate::platform::silent_command(program)
}

const SPICETIFY_VERSION: &str = "2.46.7";
const SPICETIFY_ARCHIVE_SHA256: &str = "c5a723d31eae529ba13f6922a7a027124238c7f3da7edd182a73eab03c447e9e";

fn spotify_patch_steps(root: &std::path::Path) -> &'static [&'static str] {
    if root.join("Apps/xpui.spa").is_file() { &["backup", "apply"] } else { &["apply"] }
}

#[cfg(test)]
mod integration_repair_tests {
    use super::*;

    #[test]
    fn repair_backs_up_current_stock_spotify_after_update_and_reapplies_patched_install() {
        let root = tempfile::tempdir().unwrap();
        let apps = root.path().join("Apps");
        std::fs::create_dir_all(apps.join("xpui")).unwrap();
        assert_eq!(spotify_patch_steps(root.path()), &["apply"][..]);
        std::fs::write(apps.join("xpui.spa"), b"updated client").unwrap();
        assert_eq!(spotify_patch_steps(root.path()), &["backup", "apply"][..]);
    }
}

async fn install_spicetify_cli(dir: &std::path::Path) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let url = format!(
        "https://github.com/spicetify/cli/releases/download/v{v}/spicetify-{v}-windows-x64.zip",
        v = SPICETIFY_VERSION
    );
    let client = reqwest::Client::builder().user_agent("Samplify").build().map_err(|e| e.to_string())?;
    let bytes = client.get(&url).send().await.map_err(|e| format!("download: {e}"))?
        .error_for_status().map_err(|e| format!("download: {e}"))?
        .bytes().await.map_err(|e| format!("download: {e}"))?;
    if format!("{:x}", Sha256::digest(&bytes)) != SPICETIFY_ARCHIVE_SHA256 {
        return Err("The Spicetify download did not match the release checksum.".into());
    }
    let zip_path = dir.join("cli.zip");
    std::fs::write(&zip_path, &bytes).map_err(|e| e.to_string())?;
    // Windows ships bsdtar (tar.exe), which extracts .zip — no extra crate needed.
    let out = cmd_no_window("tar")
        .args(["-xf", &zip_path.to_string_lossy(), "-C", &dir.to_string_lossy()])
        .output().map_err(|e| format!("extract: {e}"))?;
    let _ = std::fs::remove_file(zip_path);
    if !out.status.success() {
        return Err(format!("extract failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// Strip ANSI color codes (Spicetify colorizes its output) for readable errors.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until the terminating letter of the escape sequence.
            while let Some(&n) = chars.peek() {
                chars.next();
                if n.is_ascii_alphabetic() { break; }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn run_spicetify(spice: &std::path::Path, args: &[&str]) -> Result<(), String> {
    let out = cmd_no_window(spice).args(args).output()
        .map_err(|e| format!("spicetify {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        // Spicetify prints its real error to stdout, not stderr.
        let combined = format!(
            "{} {}",
            strip_ansi(&String::from_utf8_lossy(&out.stdout)),
            strip_ansi(&String::from_utf8_lossy(&out.stderr))
        );
        let detail: String = combined.split_whitespace().collect::<Vec<_>>().join(" ");
        let detail = detail.trim();
        let detail = if detail.is_empty() { format!("exit code {:?}", out.status.code()) } else { detail.to_string() };
        return Err(format!("spicetify {} failed: {}", args.join(" "), detail));
    }
    Ok(())
}

/// Which flavour of Spotify (if any) is installed. Only the standalone build from
/// spotify.com can be patched by Spicetify; the Microsoft Store (UWP/MSIX) build is
/// sandboxed and cannot be modified, so Samplify's integration won't work with it.
#[derive(Debug, PartialEq)]
pub enum SpotifyInstall {
    /// Standalone spotify.com build — path to Spotify.exe.
    Standalone(std::path::PathBuf),
    /// Microsoft Store build — present but incompatible with Spicetify.
    Store,
    /// No Spotify detected.
    None,
}

/// Detect the installed Spotify flavour. Standalone lives at
/// `%APPDATA%\Spotify\Spotify.exe`; the Store build keeps its data under
/// `%LOCALAPPDATA%\Packages\SpotifyAB.SpotifyMusic_*` and never touches %APPDATA%.
pub fn detect_spotify(appdata: &str, localappdata: &str) -> SpotifyInstall {
    let standalone = std::path::Path::new(appdata).join("Spotify").join("Spotify.exe");
    if standalone.exists() {
        return SpotifyInstall::Standalone(standalone);
    }
    let packages = std::path::Path::new(localappdata).join("Packages");
    if let Ok(entries) = std::fs::read_dir(&packages) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("SpotifyAB.SpotifyMusic") {
                return SpotifyInstall::Store;
            }
        }
    }
    SpotifyInstall::None
}

/// Lightweight probe the UI can call (e.g. on the Settings panel) to warn the user
/// up-front. Returns "standalone", "store", or "none".
#[tauri::command]
pub fn spotify_install_kind() -> Result<String, String> {
    if cfg!(target_os = "macos") { return Ok("macos".into()); }
    let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA not set".to_string())?;
    let localappdata = std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_string())?;
    Ok(match detect_spotify(&appdata, &localappdata) {
        SpotifyInstall::Standalone(_) => "standalone",
        SpotifyInstall::Store => "store",
        SpotifyInstall::None => "none",
    }
    .to_string())
}

/// One-click Spotify integration: installs Spicetify if missing, drops in the
/// bundled Samplify extension, and patches Spotify. Runs as the real user (the app
/// is a normal process), so it targets the user's own Spotify — not an elevated one.
#[tauri::command]
pub async fn setup_spotify_integration() -> Result<String, String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA not set".to_string())?;
    let localappdata = std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_string())?;

    // 1. Standalone Spotify required (Spicetify cannot patch the MS Store build).
    let spotify_exe = match detect_spotify(&appdata, &localappdata) {
        SpotifyInstall::Standalone(p) => p,
        SpotifyInstall::Store => return Err(
            "You have the Microsoft Store version of Spotify, which is sandboxed and cannot be \
             modified by Spicetify — so Samplify's in-Spotify integration won't work with it. \
             Uninstall it (Settings > Apps > Spotify > Uninstall), then install the standalone \
             installer from spotify.com and try again.".to_string()),
        SpotifyInstall::None => return Err(
            "Spotify not found. Install the standalone Spotify from spotify.com (NOT the Microsoft \
             Store version), then try again.".to_string()),
    };

    // 2. Ensure the Spicetify CLI.
    let tool_dir = std::path::Path::new(&localappdata).join("Samplify/tools").join(SPICETIFY_VERSION);
    let spice = tool_dir.join("spicetify.exe");
    if !spice.exists() {
        install_spicetify_cli(&tool_dir).await?;
    }
    if !spice.exists() {
        return Err("Could not install Spicetify.".to_string());
    }

    // 3. Write the bundled extension into Spicetify's Extensions folder.
    let ext_dir = std::path::Path::new(&appdata).join("spicetify").join("Extensions");
    std::fs::create_dir_all(&ext_dir).map_err(|e| e.to_string())?;
    // Bundled at compile time. Spotify 1.3 hashed `.main-nowPlayingBar-*`, so the
    // extension injects the Capture button via data-testid hosts, not Playbar.Button.
    std::fs::write(ext_dir.join("samplify.js"), include_str!("../../spicetify-ext/samplify.js"))
        .map_err(|e| format!("writing extension: {e}"))?;

    // Spotify can leave an old patched directory beside a newly updated stock
    // archive. Back up the current package instead of restoring an older client.
    let root = spotify_exe.parent().ok_or("Invalid Spotify install directory")?;
    run_spicetify(&spice, &["config", "spotify_path", &root.to_string_lossy()])?;
    run_spicetify(&spice, &["config", "extensions", "samplify.js"])?;
    run_spicetify(&spice, spotify_patch_steps(root))?;
    Ok("Spotify restarted. Look for Capture in the playbar. If Spotify also installed an update during restart, run the repair again.".into())
}

// Release stub: the seeder only exists in debug builds, but it's registered in
// generate_handler! unconditionally, so release builds need this to resolve.
#[cfg(not(debug_assertions))]
#[tauri::command]
pub async fn seed_dev_data(_state: State<'_, AppState>) -> Result<(), String> {
    Err("Developer seeding is only available in debug builds".to_string())
}

#[cfg(debug_assertions)]
#[tauri::command]
pub async fn seed_dev_data(state: State<'_, AppState>) -> Result<(), String> {
    let store = &state.store;
    // 1. Insert Track 1
    let t1 = store::upsert_track(
        store,
        "spotify:track:1",
        "Tides",
        "Catching Flies",
        "Tides Album",
        Some("https://i.scdn.co/image/ab67616d0000b273b5e4860d5bfa7b54a3d46fb5"),
        180000,
    ).map_err(|e| e.to_string())?;

    // Write a dummy WAV file for Session 1
    let clips_dir = state.config.lock().unwrap().clips_dir.clone();
    let _ = std::fs::create_dir_all(&clips_dir);
    let file_path = clips_dir.join("Catching Flies - Tides [Seed].wav");
    let file_path_str = file_path.to_string_lossy().to_string();
    
    // Write 3-second stereo WAV
    let samples_count = 44100 * 2 * 3;
    let mut samples = vec![0.0f32; samples_count];
    for i in 0..samples_count {
        if i > samples_count / 2 - 1000 && i < samples_count / 2 + 1000 {
            samples[i] = (i as f32 * 0.1).sin() * 0.8;
        } else {
            samples[i] = (i as f32 * 0.05).sin() * 0.2;
        }
    }
    crate::session::write_wav_i16(&file_path, &samples, 44100, 2).map_err(|e| e.to_string())?;

    let s1 = store::insert_session(store, t1, &file_path_str, 0, 44100, 2).map_err(|e| e.to_string())?;

    let m1 = store::insert_mark(store, t1, 30000, Some(s1), Some(30000), "Dope drum break chop").map_err(|e| e.to_string())?;
    store::set_mark_text_tags(store, m1, "Dope drum break chop", &["break".to_string()]).map_err(|e| e.to_string())?;
    store::update_mark_meta(store, m1, "Dope drum break chop", &["break".to_string()], Some(5), Some("Am".to_string()), Some(90)).map_err(|e| e.to_string())?;

    let m2 = store::insert_mark(store, t1, 60000, Some(s1), Some(60000), "Dreamy synth hook").map_err(|e| e.to_string())?;
    store::set_mark_text_tags(store, m2, "Dreamy synth hook", &["melody".to_string()]).map_err(|e| e.to_string())?;
    store::update_mark_meta(store, m2, "Dreamy synth hook", &["melody".to_string()], Some(4), Some("C".to_string()), Some(90)).map_err(|e| e.to_string())?;

    // 2. Insert Track 2
    let t2 = store::upsert_track(
        store,
        "spotify:track:2",
        "Vocal Flip",
        "Logic",
        "Viny",
        Some("https://i.scdn.co/image/ab67616d0000b2735d5e56e0d5a498b58402c46f"),
        240000,
    ).map_err(|e| e.to_string())?;

    let file_path2 = clips_dir.join("Logic - Vocal Flip [Seed].wav");
    let file_path_str2 = file_path2.to_string_lossy().to_string();
    crate::session::write_wav_i16(&file_path2, &samples, 44100, 2).map_err(|e| e.to_string())?;

    let s2 = store::insert_session(store, t2, &file_path_str2, 0, 44100, 2).map_err(|e| e.to_string())?;
    let m3 = store::insert_mark(store, t2, 120000, Some(s2), Some(120000), "Vocal chop sample").map_err(|e| e.to_string())?;
    store::set_mark_text_tags(store, m3, "Vocal chop sample", &["vocal".to_string()]).map_err(|e| e.to_string())?;
    store::update_mark_meta(store, m3, "Vocal chop sample", &["vocal".to_string()], Some(3), Some("G".to_string()), Some(110)).map_err(|e| e.to_string())?;

    Ok(())
}

#[derive(serde::Serialize)]
pub struct CaptureStatus {
    /// True when the system-audio capture backend is authorized and ready
    /// to push samples into the ring buffer. False on macOS until the user
    /// grants "Screen Recording" permission; always true on Windows.
    pub authorized: bool,
    /// "windows" | "macos" — surfaced so the frontend can show the macOS-only
    /// permission banner / Settings row and hide them on Windows.
    pub platform: String,
    /// Human-readable reason when `authorized` is false (e.g. "Denied — open
    /// System Settings to grant Screen Recording and restart Samplify").
    pub denied_reason: Option<String>,
}

/// Reports the system-audio capture backend's permission state. On macOS the
/// frontend uses this to gate the Library empty-state banner + Settings row
/// ("Grant Screen Recording permission" / "Open System Settings"). On Windows
/// this always returns `authorized: true` (WASAPI loopback needs no grant).
/// `platform` is also returned so a single JS bundle can branch without
/// sniffing userAgent.
#[tauri::command]
pub fn check_capture_status() -> CaptureStatus {
    let auth = crate::capture::authorization();
    let platform: String = if cfg!(target_os = "macos") { "macos" } else { "windows" }.into();
    let denied_reason = match auth {
        crate::capture::CaptureAuth::Authorized => None,
        crate::capture::CaptureAuth::NotDetermined => Some(
            "Samplify needs Screen Recording permission to capture audio. Grant it and restart.".into(),
        ),
        crate::capture::CaptureAuth::Denied => Some(
            "Screen Recording permission was denied. Open System Settings \u{2192} Privacy & Security \u{2192} Screen Recording, enable Samplify, then restart.".into(),
        ),
        crate::capture::CaptureAuth::Restricted => Some(
            "Capture is restricted by a configuration profile (MDM) on this Mac.".into(),
        ),
    };
    CaptureStatus { authorized: auth == crate::capture::CaptureAuth::Authorized, platform, denied_reason }
}

/// Opens the OS's Screen Recording privacy pane on macOS. No-op on Windows
/// (WASAPI loopback needs no permission). Wired to the banner's "Open System
/// Settings" button in the frontend.
#[tauri::command]
pub fn open_screen_recording_settings() -> Result<(), String> {
    crate::platform::open_screen_recording_settings().map_err(|e| e.to_string())
}

#[cfg(test)]
mod spotify_detect_tests {
    use super::{detect_spotify, SpotifyInstall};
    use std::fs;

    #[test]
    fn detects_standalone_by_exe() {
        let ad = tempfile::tempdir().unwrap();
        let lad = tempfile::tempdir().unwrap();
        let sp = ad.path().join("Spotify");
        fs::create_dir_all(&sp).unwrap();
        fs::write(sp.join("Spotify.exe"), b"x").unwrap();
        assert!(matches!(
            detect_spotify(ad.path().to_str().unwrap(), lad.path().to_str().unwrap()),
            SpotifyInstall::Standalone(_)
        ));
    }

    #[test]
    fn detects_store_package() {
        let ad = tempfile::tempdir().unwrap();
        let lad = tempfile::tempdir().unwrap();
        fs::create_dir_all(lad.path().join("Packages").join("SpotifyAB.SpotifyMusic_zpdnekdrzrea0")).unwrap();
        assert_eq!(
            detect_spotify(ad.path().to_str().unwrap(), lad.path().to_str().unwrap()),
            SpotifyInstall::Store
        );
    }

    #[test]
    fn none_when_absent() {
        let ad = tempfile::tempdir().unwrap();
        let lad = tempfile::tempdir().unwrap();
        assert_eq!(
            detect_spotify(ad.path().to_str().unwrap(), lad.path().to_str().unwrap()),
            SpotifyInstall::None
        );
    }

    #[test]
    fn standalone_wins_over_store() {
        let ad = tempfile::tempdir().unwrap();
        let lad = tempfile::tempdir().unwrap();
        let sp = ad.path().join("Spotify");
        fs::create_dir_all(&sp).unwrap();
        fs::write(sp.join("Spotify.exe"), b"x").unwrap();
        fs::create_dir_all(lad.path().join("Packages").join("SpotifyAB.SpotifyMusic_x")).unwrap();
        assert!(matches!(
            detect_spotify(ad.path().to_str().unwrap(), lad.path().to_str().unwrap()),
            SpotifyInstall::Standalone(_)
        ));
    }
}
