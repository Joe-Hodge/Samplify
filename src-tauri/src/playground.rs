use crate::{state::AppState, store::Store};
use anyhow::{ensure, Result};
use base64::Engine;
use rusqlite::params;
use std::{
    io::{Cursor, Write},
    path::Path,
};
use tauri::{
    ipc::{InvokeBody, Request},
    Emitter,
};

enum TakeKind {
    Import,
    Mix,
}

// Imported files and rendered mixes enter the same library and export workflow as
// captures. Web Audio decodes the source format and sends one PCM WAV over binary IPC.
fn save_take(store: &Store, dir: &Path, title: &str, kind: TakeKind, bytes: &[u8]) -> Result<i64> {
    ensure!(
        !title.trim().is_empty() && title.chars().count() <= 200,
        "Give the clip a name of up to 200 characters"
    );
    ensure!(
        bytes.len() <= 128 * 1024 * 1024,
        "Clip exceeds the 128 MB PCM limit"
    );
    let wav = hound::WavReader::new(Cursor::new(bytes))?;
    let spec = wav.spec();
    ensure!(
        spec.channels > 0
            && spec.channels <= 2
            && (8000..=192000).contains(&spec.sample_rate)
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "Expected mono or stereo PCM16 audio"
    );
    let frames = u64::from(wav.duration());
    let payload = u64::from(wav.len()) * 2;
    ensure!(
        frames > 0
            && frames * u64::from(spec.channels) * 2 == payload
            && wav.into_inner().position() + payload == bytes.len() as u64,
        "Incomplete PCM audio"
    );
    let duration_ms = frames * 1000 / u64::from(spec.sample_rate);
    let artist = match kind {
        TakeKind::Import => "Imported audio",
        TakeKind::Mix => "Playground mix",
    };
    let id = uuid::Uuid::new_v4();
    let path = dir.join(format!("Playground-{id}.wav"));
    std::fs::create_dir_all(dir)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let result = (|| -> Result<i64> {
        file.write_all(bytes)?;
        file.sync_all()?;
        let mut conn = store.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let time = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO tracks(spotify_id,title,artist,album,duration_ms) VALUES (?1,?2,?3,'',?4)",
            params![format!("local:{id}"), title, artist, duration_ms],
        )?;
        let track = tx.last_insert_rowid();
        tx.execute("INSERT INTO sessions(track_id,file_path,file_start_track_ms,sample_rate,channels,created_at) VALUES (?1,?2,0,?3,?4,?5)",
            params![track,path.to_string_lossy(),spec.sample_rate,spec.channels,time])?;
        tx.execute("INSERT INTO marks(track_id,session_id,track_ms,session_offset_ms,note_text,created_at,updated_at) VALUES (?1,?2,0,0,?3,?4,?4)",
            params![track,tx.last_insert_rowid(),title,time])?;
        let mark = tx.last_insert_rowid();
        tx.commit()?;
        Ok(mark)
    })();
    drop(file);
    if result.is_err() {
        std::fs::remove_file(path)?;
    }
    result
}

#[tauri::command]
pub async fn save_playground_take(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: Request<'_>,
) -> Result<i64, String> {
    let header = |key: &str| {
        request
            .headers()
            .get(key)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    };
    let title = base64::engine::general_purpose::STANDARD
        .decode(header("x-samplify-title"))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or("Invalid clip title")?;
    let kind = match header("x-samplify-kind") {
        "import" => TakeKind::Import,
        "mix" => TakeKind::Mix,
        _ => return Err("Unknown take type".into()),
    };
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("Expected binary WAV data".into());
    };
    let dir = state.config.lock().unwrap().clips_dir.clone();
    let id = save_take(&state.store, &dir, &title, kind, bytes).map_err(|e| e.to_string())?;
    let _ = app.emit(
        "mark-created",
        serde_json::json!({"mark_id": id, "want_popup": false}),
    );
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav = hound::WavWriter::new(&mut bytes, spec).unwrap();
        for _ in 0..480 {
            wav.write_sample(1000i16).unwrap();
            wav.write_sample(-500i16).unwrap();
        }
        wav.finalize().unwrap();
        bytes.into_inner()
    }

    #[test]
    fn imported_audio_and_bounce_are_independent_searchable_playable_notes() {
        let store = crate::store::Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bytes = wav();
        let a = save_take(&store, dir.path(), "Pulse & kick", TakeKind::Import, &bytes).unwrap();
        let b = save_take(&store, dir.path(), "Pulse & kick", TakeKind::Mix, &bytes).unwrap();
        let clips = crate::store::get_marks_by_ids(&store, &[a, b]).unwrap();
        assert_eq!(clips.len(), 2);
        assert_ne!(clips[0].file_path, clips[1].file_path);
        assert_eq!(clips[0].title, "Pulse & kick");
        assert_eq!(clips[1].artist, "Playground mix");
        assert_eq!(std::fs::read(&clips[0].file_path).unwrap(), bytes);
        let duration: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT duration_ms FROM tracks LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(duration, 10);
        assert_eq!(
            crate::store::search_marks(&store, "Pulse").unwrap().len(),
            2
        );
    }

    #[test]
    fn corrupt_or_uncommitted_take_leaves_no_artifacts() {
        let store = crate::store::Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut damaged = wav();
        damaged.truncate(damaged.len() - 4);
        assert!(save_take(&store, dir.path(), "Cut", TakeKind::Import, &damaged).is_err());
        store.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_mark BEFORE INSERT ON marks BEGIN SELECT RAISE(ABORT,'failed'); END;").unwrap();
        assert!(save_take(&store, dir.path(), "Mix", TakeKind::Mix, &wav()).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let n: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
}
