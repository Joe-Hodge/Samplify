use crate::state::AppState;
use crate::{session, store};
use anyhow::Result;

pub struct MarkOutcome {
    pub mark_id: i64,
    pub title: String,
    pub track_ms: i64,
    pub has_audio: bool,
}

/// Creates a mark at the current clock position.
/// Defers audio snapshot compilation to SessionState Finalize time.
pub fn create_mark(app: &AppState, now_ms: i64, current: Option<&crate::spotify::NowPlaying>) -> Result<MarkOutcome> {
    // Resolve the track + timestamp. Prefer the clock; fall back to `current`.
    let pos = app.clock.lock().unwrap().position(now_ms);
    let (spotify_id, track_ms) = match (pos, current) {
        (Some((id, ms)), _) if !id.is_empty() => (id, ms),
        (_, Some(np)) if np.is_track => (np.spotify_id.clone(), np.progress_ms),
        _ => return Err(anyhow::anyhow!("nothing playing on this device")),
    };

    // Upsert the track (needs metadata; `current` supplies it when available).
    let (title, artist, album, art, dur) = match current {
        Some(np) => (np.title.clone(), np.artist.clone(), np.album.clone(), np.album_art_url.clone(), np.duration_ms),
        None => (spotify_id.clone(), String::new(), String::new(), None, 0),
    };
    let track_id = store::upsert_track(&app.store, &spotify_id, &title, &artist, &album, art.as_deref(), dur)?;

    // 1. Insert the mark immediately into the database with NULL session details.
    let mark_id = store::insert_mark(&app.store, track_id, track_ms, None, None, "")?;

    // 2. Track the active session state in-memory.
    let (_action, finalize) = {
        let mut ss = app.sessions.lock().unwrap();
        ss.on_mark(&spotify_id, now_ms, track_ms, mark_id)
    };

    // 3. If the state machine requires finalizing the old session, do it now.
    if let Some(fin) = finalize {
        let _ = finalize_session(app, fin);
    }

    // Capture is active if we have audio stream format initialized.
    let has_audio = {
        let rb = app.ring.lock().unwrap();
        rb.sample_rate > 0 && !rb.is_empty()
    };

    Ok(MarkOutcome { mark_id, title, track_ms, has_audio })
}

/// Extracts a range from the circular buffer covering the session window,
/// writes the WAV file to disk, and updates the database rows.
pub fn finalize_session(app: &AppState, fin: session::Finalize) -> Result<()> {
    if fin.marks.is_empty() {
        return Ok(());
    }

    let (config_pre, config_post, clips_dir) = {
        let c = app.config.lock().unwrap();
        (c.pre_roll, c.post_roll, c.clips_dir.clone())
    };

    // Extract window from ring buffer
    let (sr, ch, samples, start_wc) = {
        let rb = app.ring.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let start_wc = fin.first_mark_wallclock - (config_pre * 1000.0) as i64;
        let end_wc = fin.last_mark_wallclock + (config_post * 1000.0) as i64;
        let samples = rb.get_range(now, start_wc, end_wc);
        (rb.sample_rate, rb.channels, samples, start_wc)
    };

    if samples.is_empty() || sr == 0 {
        return Ok(());
    }

    // Retrieve database track_id from the first mark in the session.
    let track_id: i64 = app.store.conn.lock().unwrap().query_row(
        "SELECT track_id FROM marks WHERE id = ?1",
        rusqlite::params![fin.marks[0].mark_id],
        |r| r.get(0)
    )?;

    let track = store::get_track_by_id(&app.store, track_id)?;
    let title = track.title;
    let artist = track.artist;

    let ts = chrono::Local::now().format("%Y-%m-%d %H%M%S").to_string();
    let fname = session::sanitize_filename(&artist, &title, &ts);
    let path = clips_dir.join(&fname);

    if session::write_wav_i16(&path, &samples, sr, ch).is_ok() {
        // Calculate the actual file_start_track_ms.
        let now_ms = chrono::Utc::now().timestamp_millis();
        let (buf_len, start_idx) = {
            let rb = app.ring.lock().unwrap();
            let start_ago = (now_ms - start_wc).max(0) as f64 / 1000.0;
            // Frame-aligned, matching RingBuffer::get_range.
            let start_offset = (start_ago * sr as f64).round() as usize * ch as usize;
            let len = rb.len();
            let start_idx = len.saturating_sub(start_offset);
            (len, start_idx)
        };
        
        let start_age_ms = ((buf_len - start_idx) as f64 / ch as f64 / sr as f64 * 1000.0) as i64;
        let file_start_wallclock = now_ms - start_age_ms;
        
        let actual_seeded_ms = fin.first_mark_wallclock - file_start_wallclock;
        
        let first_mark = &fin.marks[0];
        let file_start_track_ms = (first_mark.track_ms - actual_seeded_ms).max(0);

        // Insert session row
        let sid = store::insert_session(
            &app.store,
            track_id,
            &path.to_string_lossy(),
            file_start_track_ms,
            sr as i64,
            ch as i64,
        )?;

        // Update each mark's session_id and session_offset_ms.
        for mark in &fin.marks {
            let offset_ms = mark.track_ms - file_start_track_ms;
            store::update_mark_session(&app.store, mark.mark_id, sid, offset_ms)?;
        }
    }

    Ok(())
}
