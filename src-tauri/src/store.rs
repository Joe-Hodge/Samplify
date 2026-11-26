use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    pub conn: Mutex<Connection>,
}

#[derive(serde::Serialize, Clone)]
pub struct MarkRow {
    pub id: i64,
    pub track_id: i64,
    pub track_ms: i64,
    pub note_text: String,
    pub tags: Vec<String>,
    pub title: String,
    pub artist: String,
    pub created_at: String,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tracks (
  id INTEGER PRIMARY KEY,
  spotify_id TEXT UNIQUE,
  title TEXT NOT NULL,
  artist TEXT NOT NULL,
  album TEXT,
  album_art_url TEXT,
  duration_ms INTEGER,
  favorite INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS sessions (
  id INTEGER PRIMARY KEY,
  track_id INTEGER NOT NULL REFERENCES tracks(id),
  file_path TEXT NOT NULL,
  file_start_track_ms INTEGER,
  sample_rate INTEGER,
  channels INTEGER,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS marks (
  id INTEGER PRIMARY KEY,
  track_id INTEGER NOT NULL REFERENCES tracks(id),
  session_id INTEGER REFERENCES sessions(id),
  track_ms INTEGER NOT NULL,
  session_offset_ms INTEGER,
  note_text TEXT NOT NULL DEFAULT '',
  trim_start_ms INTEGER, trim_end_ms INTEGER,
  rating INTEGER, key TEXT, bpm INTEGER, used_in TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tags (
  id INTEGER PRIMARY KEY,
  name TEXT UNIQUE NOT NULL,
  color TEXT,
  sort_order INTEGER
);
CREATE TABLE IF NOT EXISTS mark_tags (
  mark_id INTEGER NOT NULL REFERENCES marks(id) ON DELETE CASCADE,
  tag_id INTEGER NOT NULL REFERENCES tags(id),
  PRIMARY KEY (mark_id, tag_id)
);
CREATE TABLE IF NOT EXISTS collections (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS collection_marks (
  collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  mark_id INTEGER NOT NULL REFERENCES marks(id) ON DELETE CASCADE,
  added_at TEXT NOT NULL,
  PRIMARY KEY (collection_id, mark_id)
);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);

CREATE VIRTUAL TABLE IF NOT EXISTS marks_fts USING fts5(note_text, content='marks', content_rowid='id');
CREATE TRIGGER IF NOT EXISTS marks_ai AFTER INSERT ON marks BEGIN
  INSERT INTO marks_fts(rowid, note_text) VALUES (new.id, new.note_text);
END;
CREATE TRIGGER IF NOT EXISTS marks_au AFTER UPDATE ON marks BEGIN
  INSERT INTO marks_fts(marks_fts, rowid, note_text) VALUES('delete', old.id, old.note_text);
  INSERT INTO marks_fts(rowid, note_text) VALUES (new.id, new.note_text);
END;
CREATE TRIGGER IF NOT EXISTS marks_ad AFTER DELETE ON marks BEGIN
  INSERT INTO marks_fts(marks_fts, rowid, note_text) VALUES('delete', old.id, old.note_text);
END;
"#;

const SEED_TAGS: &[(&str, &str)] = &[
    ("break", "#E8912D"), ("vocal", "#5FB878"), ("melody", "#A87CD8"),
    ("bass", "#E5736B"), ("chords", "#E0B84C"), ("texture", "#5F9FD8"), ("fx", "#5B8FF0"),
];

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }
    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(conn: Connection) -> Result<Store> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        // Migrations for databases created before a column existed. ALTER errors
        // ("duplicate column") are expected on already-migrated DBs — ignore them.
        let _ = conn.execute("ALTER TABLE tracks ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0", []);
        // Stem provenance: which mark a stem was split from, and which stem it is.
        // Both NULL for ordinary notes. Additive and nullable, so existing captures
        // are untouched.
        let _ = conn.execute("ALTER TABLE marks ADD COLUMN source_mark_id INTEGER", []);
        let _ = conn.execute("ALTER TABLE marks ADD COLUMN stem TEXT", []);
        for (i, (name, color)) in SEED_TAGS.iter().enumerate() {
            conn.execute(
                "INSERT OR IGNORE INTO tags(name, color, sort_order) VALUES (?1, ?2, ?3)",
                params![name, color, i as i64],
            )?;
        }
        Ok(Store { conn: Mutex::new(conn) })
    }
}

fn now() -> String { chrono::Utc::now().to_rfc3339() }

pub fn upsert_track(s: &Store, spotify_id: &str, title: &str, artist: &str,
    album: &str, album_art_url: Option<&str>, duration_ms: i64) -> Result<i64> {
    let c = s.conn.lock().unwrap();
    c.execute(
        "INSERT INTO tracks(spotify_id,title,artist,album,album_art_url,duration_ms)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(spotify_id) DO UPDATE SET title=?2, artist=?3, album=?4, album_art_url=?5, duration_ms=?6",
        params![spotify_id, title, artist, album, album_art_url, duration_ms],
    )?;
    Ok(c.query_row("SELECT id FROM tracks WHERE spotify_id = ?1", params![spotify_id], |r| r.get(0))?)
}

pub fn insert_mark(s: &Store, track_id: i64, track_ms: i64,
    session_id: Option<i64>, session_offset_ms: Option<i64>, note_text: &str) -> Result<i64> {
    let c = s.conn.lock().unwrap();
    let t = now();
    c.execute(
        "INSERT INTO marks(track_id,session_id,track_ms,session_offset_ms,note_text,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?6)",
        params![track_id, session_id, track_ms, session_offset_ms, note_text, t],
    )?;
    Ok(c.last_insert_rowid())
}

/// Insert a new mark carved from an existing capture's waveform.
///
/// Writes NO audio: the new mark points at the same `sessions` row with its own
/// trim, so one capture can be mined for several loops without touching the WAV.
/// `track_id` is read from the session rather than passed in, so a caller cannot
/// desync the two.
pub fn create_mark_from_selection(
    s: &Store,
    session_id: i64,
    trim_start_ms: i64,
    trim_end_ms: i64,
    note_text: &str,
    tag_names: &[String],
) -> Result<i64> {
    if trim_start_ms < 0 {
        anyhow::bail!("selection starts before the clip");
    }
    if trim_end_ms <= trim_start_ms {
        anyhow::bail!("selection is empty");
    }

    // Scoped so the lock is released before insert_mark/set_mark_text_tags take it.
    let (track_id, file_start_track_ms, file_path) = {
        let c = s.conn.lock().unwrap();
        c.query_row(
            "SELECT track_id, file_start_track_ms, file_path FROM sessions WHERE id = ?1",
            params![session_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?)),
        )
        .map_err(|_| anyhow::anyhow!("unknown capture session {session_id}"))?
    };

    // Clamp to the capture's real length so a selection can never point past the WAV.
    let end_ms = match get_wav_duration_ms(&file_path) {
        Some(dur) if trim_end_ms > dur => dur,
        _ => trim_end_ms,
    };
    if end_ms <= trim_start_ms {
        anyhow::bail!("selection is empty");
    }

    let track_ms = file_start_track_ms.unwrap_or(0) + trim_start_ms;
    let mark_id = insert_mark(s, track_id, track_ms, Some(session_id), Some(trim_start_ms), note_text)?;

    {
        let c = s.conn.lock().unwrap();
        c.execute(
            "UPDATE marks SET trim_start_ms=?1, trim_end_ms=?2 WHERE id=?3",
            params![trim_start_ms, end_ms, mark_id],
        )?;
    }
    set_mark_text_tags(s, mark_id, note_text, tag_names)?;
    Ok(mark_id)
}

pub fn set_mark_text_tags(s: &Store, mark_id: i64, note_text: &str, tag_names: &[String]) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute("UPDATE marks SET note_text=?1, updated_at=?2 WHERE id=?3", params![note_text, now(), mark_id])?;
    c.execute("DELETE FROM mark_tags WHERE mark_id=?1", params![mark_id])?;
    for name in tag_names {
        let tag_id: i64 = c.query_row("SELECT id FROM tags WHERE name=?1", params![name], |r| r.get(0))?;
        c.execute("INSERT OR IGNORE INTO mark_tags(mark_id, tag_id) VALUES (?1,?2)", params![mark_id, tag_id])?;
    }
    Ok(())
}

fn tags_for(c: &Connection, mark_id: i64) -> Result<Vec<String>> {
    let mut stmt = c.prepare(
        "SELECT t.name FROM mark_tags mt JOIN tags t ON t.id = mt.tag_id WHERE mt.mark_id = ?1 ORDER BY t.sort_order")?;
    let rows = stmt.query_map(params![mark_id], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn row_to_mark(c: &Connection, id: i64, track_id: i64, track_ms: i64,
    note_text: String, title: String, artist: String, created_at: String) -> Result<MarkRow> {
    Ok(MarkRow { id, track_id, track_ms, note_text, tags: tags_for(c, id)?, title, artist, created_at })
}

pub fn list_marks(s: &Store) -> Result<Vec<MarkRow>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare(
        "SELECT m.id, m.track_id, m.track_ms, m.note_text, t.title, t.artist, m.created_at
         FROM marks m JOIN tracks t ON t.id = m.track_id ORDER BY m.created_at DESC")?;
    let ids: Vec<(i64,i64,i64,String,String,String,String)> = stmt.query_map([], |r| Ok((
        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .filter_map(|r| r.ok()).collect();
    ids.into_iter().map(|(id,tid,tms,nt,ti,ar,ca)| row_to_mark(&c, id, tid, tms, nt, ti, ar, ca)).collect()
}

pub fn search_marks(s: &Store, query: &str) -> Result<Vec<MarkRow>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare(
        "SELECT m.id, m.track_id, m.track_ms, m.note_text, t.title, t.artist, m.created_at
         FROM marks_fts f JOIN marks m ON m.id = f.rowid JOIN tracks t ON t.id = m.track_id
         WHERE marks_fts MATCH ?1 ORDER BY rank")?;
    let ids: Vec<(i64,i64,i64,String,String,String,String)> = stmt.query_map(params![query], |r| Ok((
        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .filter_map(|r| r.ok()).collect();
    ids.into_iter().map(|(id,tid,tms,nt,ti,ar,ca)| row_to_mark(&c, id, tid, tms, nt, ti, ar, ca)).collect()
}

pub fn get_setting(s: &Store, key: &str) -> Result<Option<String>> {
    let c = s.conn.lock().unwrap();
    Ok(c.query_row("SELECT value FROM settings WHERE key=?1", params![key], |r| r.get(0)).ok())
}
pub fn set_setting(s: &Store, key: &str, value: &str) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute("INSERT INTO settings(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
        params![key, value])?;
    Ok(())
}
pub fn tag_names(s: &Store) -> Result<Vec<String>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare("SELECT name FROM tags ORDER BY sort_order")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn insert_session(s: &Store, track_id: i64, file_path: &str,
    file_start_track_ms: i64, sample_rate: i64, channels: i64) -> Result<i64> {
    let c = s.conn.lock().unwrap();
    c.execute(
        "INSERT INTO sessions(track_id,file_path,file_start_track_ms,sample_rate,channels,created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![track_id, file_path, file_start_track_ms, sample_rate, channels, now()],
    )?;
    Ok(c.last_insert_rowid())
}

pub fn latest_session_for_track(s: &Store, track_id: i64) -> Result<Option<i64>> {
    let c = s.conn.lock().unwrap();
    Ok(c.query_row("SELECT id FROM sessions WHERE track_id=?1 ORDER BY id DESC LIMIT 1",
        params![track_id], |r| r.get(0)).ok())
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct LibraryTrack {
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub album_art_url: Option<String>,
    pub mark_count: i64,
    pub latest_mark_at: String,
    pub favorite: bool,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct TrackRow {
    pub id: i64,
    pub spotify_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_art_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub favorite: bool,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct MarkDetail {
    pub id: i64,
    pub track_ms: i64,
    pub session_id: Option<i64>,
    pub session_offset_ms: Option<i64>,
    pub note_text: String,
    pub tags: Vec<String>,
    pub rating: Option<i64>,
    pub key: Option<String>,
    pub bpm: Option<i64>,
    pub trim_start_ms: Option<i64>,
    pub trim_end_ms: Option<i64>,
    /// Which stem this note is, when it came from a split. None for ordinary notes.
    pub stem: Option<String>,
    /// The mark this stem was split from. None for ordinary notes.
    pub source_mark_id: Option<i64>,
    pub created_at: String,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct SessionRow {
    pub id: i64,
    pub file_path: String,
    pub file_start_track_ms: Option<i64>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct TrackDetail {
    pub track: TrackRow,
    pub marks: Vec<MarkDetail>,
    pub sessions: Vec<SessionRow>,
}

pub fn get_library(s: &Store) -> Result<Vec<LibraryTrack>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare(
        "SELECT t.id, t.title, t.artist, t.album_art_url, COUNT(m.id) as mark_count, MAX(m.created_at) as latest_mark_at, t.favorite
         FROM tracks t
         JOIN marks m ON m.track_id = t.id
         GROUP BY t.id
         ORDER BY latest_mark_at DESC"
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(LibraryTrack {
            track_id: r.get(0)?,
            title: r.get(1)?,
            artist: r.get(2)?,
            album_art_url: r.get(3)?,
            mark_count: r.get(4)?,
            latest_mark_at: r.get(5)?,
            favorite: r.get::<_, i64>(6)? != 0,
        })
    })?;
    let mut res = Vec::new();
    for r in rows {
        res.push(r?);
    }
    Ok(res)
}

/// Library tracks that have at least one mark carrying the given tag.
pub fn tracks_by_tag(s: &Store, tag: &str) -> Result<Vec<LibraryTrack>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare(
        "SELECT t.id, t.title, t.artist, t.album_art_url, COUNT(DISTINCT m.id), MAX(m.created_at), t.favorite
         FROM tracks t
         JOIN marks m ON m.track_id = t.id
         JOIN mark_tags mt ON mt.mark_id = m.id
         JOIN tags tg ON tg.id = mt.tag_id
         WHERE tg.name = ?1
         GROUP BY t.id
         ORDER BY MAX(m.created_at) DESC"
    )?;
    let rows = stmt.query_map(params![tag], |r| {
        Ok(LibraryTrack {
            track_id: r.get(0)?,
            title: r.get(1)?,
            artist: r.get(2)?,
            album_art_url: r.get(3)?,
            mark_count: r.get(4)?,
            latest_mark_at: r.get(5)?,
            favorite: r.get::<_, i64>(6)? != 0,
        })
    })?;
    let mut res = Vec::new();
    for r in rows {
        res.push(r?);
    }
    Ok(res)
}

// ---- Collections (user-organized folders of clips) ----

#[derive(serde::Serialize, Clone, Debug)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub clip_count: i64,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct CollectionClip {
    pub mark_id: i64,
    pub title: String,
    pub artist: String,
    pub note_text: String,
    pub track_ms: i64,
    pub duration_ms: Option<i64>,
    pub rating: Option<i64>,
    pub tags: Vec<String>,
    pub file_path: String,
    pub trim_start_ms: Option<i64>,
    pub trim_end_ms: Option<i64>,
    pub album_art_url: Option<String>,
}

pub fn create_collection(s: &Store, name: &str) -> Result<i64> {
    let c = s.conn.lock().unwrap();
    c.execute("INSERT INTO collections(name, created_at) VALUES (?1, ?2)", params![name, now()])?;
    Ok(c.last_insert_rowid())
}

pub fn delete_collection(s: &Store, id: i64) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute("DELETE FROM collections WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn rename_collection(s: &Store, id: i64, name: &str) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute("UPDATE collections SET name = ?1 WHERE id = ?2", params![name, id])?;
    Ok(())
}

pub fn list_collections(s: &Store) -> Result<Vec<Collection>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare(
        "SELECT c.id, c.name, COUNT(cm.mark_id)
         FROM collections c LEFT JOIN collection_marks cm ON cm.collection_id = c.id
         GROUP BY c.id ORDER BY c.name COLLATE NOCASE")?;
    let rows = stmt.query_map([], |r| Ok(Collection { id: r.get(0)?, name: r.get(1)?, clip_count: r.get(2)? }))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn set_mark_in_collection(s: &Store, collection_id: i64, mark_id: i64, member: bool) -> Result<()> {
    let c = s.conn.lock().unwrap();
    if member {
        c.execute("INSERT OR IGNORE INTO collection_marks(collection_id, mark_id, added_at) VALUES (?1, ?2, ?3)",
            params![collection_id, mark_id, now()])?;
    } else {
        c.execute("DELETE FROM collection_marks WHERE collection_id = ?1 AND mark_id = ?2",
            params![collection_id, mark_id])?;
    }
    Ok(())
}

/// The collection ids a given mark belongs to (for the note editor's checkboxes).
pub fn collections_for_mark(s: &Store, mark_id: i64) -> Result<Vec<i64>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare("SELECT collection_id FROM collection_marks WHERE mark_id = ?1")?;
    let rows = stmt.query_map(params![mark_id], |r| r.get::<_, i64>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn collection_clips(s: &Store, collection_id: i64) -> Result<Vec<CollectionClip>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare(
        "SELECT m.id, t.title, t.artist, m.note_text, m.track_ms, m.rating,
                se.file_path, m.trim_start_ms, m.trim_end_ms, t.album_art_url
         FROM collection_marks cm
         JOIN marks m ON m.id = cm.mark_id
         JOIN tracks t ON t.id = m.track_id
         JOIN sessions se ON se.id = m.session_id
         WHERE cm.collection_id = ?1
         ORDER BY cm.added_at ASC")?;
    let rows: Vec<(i64, String, String, String, i64, Option<i64>, String, Option<i64>, Option<i64>, Option<String>)> =
        stmt.query_map(params![collection_id], |r| Ok((
            r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?,
        )))?.filter_map(|r| r.ok()).collect();
    Ok(rows.into_iter().map(|(mark_id, title, artist, note_text, track_ms, rating, file_path, trim_start_ms, trim_end_ms, album_art_url)| {
        let tags = tags_for(&c, mark_id).unwrap_or_default();
        let duration_ms = get_wav_duration_ms(&file_path);
        CollectionClip { mark_id, title, artist, note_text, track_ms, duration_ms, rating, tags, file_path, trim_start_ms, trim_end_ms, album_art_url }
    }).collect())
}

/// A clip payload for the Playground tab. Read-only view over marks/sessions.
#[derive(serde::Serialize, Clone, Debug)]
pub struct PlaygroundClip {
    pub mark_id: i64,
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub note_text: String,
    pub tags: Vec<String>,
    pub rating: Option<i64>,
    pub key: Option<String>,
    pub bpm: Option<i64>,
    pub file_path: String,
    pub trim_start_ms: Option<i64>,
    pub trim_end_ms: Option<i64>,
    pub duration_ms: Option<i64>,
}

/// Fetch clips for the given mark ids, preserving input order. Unknown ids and
/// marks without a capture session are skipped (used by Playground restore,
/// where stale saved ids are expected after deletes).
pub fn get_marks_by_ids(s: &Store, ids: &[i64]) -> Result<Vec<PlaygroundClip>> {
    let c = s.conn.lock().unwrap();
    let mut out = Vec::new();
    for &id in ids {
        let row = c.query_row(
            "SELECT m.id, t.id, t.title, t.artist, m.note_text, m.rating, m.key, m.bpm,
                    se.file_path, m.trim_start_ms, m.trim_end_ms
             FROM marks m
             JOIN tracks t ON t.id = m.track_id
             JOIN sessions se ON se.id = m.session_id
             WHERE m.id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, Option<i64>>(9)?,
                    r.get::<_, Option<i64>>(10)?,
                ))
            },
        );
        if let Ok((mark_id, track_id, title, artist, note_text, rating, key, bpm, file_path, trim_start_ms, trim_end_ms)) = row {
            let tags = tags_for(&c, mark_id).unwrap_or_default();
            let duration_ms = get_wav_duration_ms(&file_path);
            out.push(PlaygroundClip {
                mark_id, track_id, title, artist, note_text, tags, rating, key, bpm,
                file_path, trim_start_ms, trim_end_ms, duration_ms,
            });
        }
    }
    Ok(out)
}

pub fn get_track_detail(s: &Store, track_id: i64) -> Result<TrackDetail> {
    let c = s.conn.lock().unwrap();
    let track = c.query_row(
        "SELECT id, spotify_id, title, artist, album, album_art_url, duration_ms, favorite
         FROM tracks WHERE id = ?1",
        params![track_id],
        |r| {
            Ok(TrackRow {
                id: r.get(0)?,
                spotify_id: r.get(1)?,
                title: r.get(2)?,
                artist: r.get(3)?,
                album: r.get(4)?,
                album_art_url: r.get(5)?,
                duration_ms: r.get(6)?,
                favorite: r.get::<_, i64>(7)? != 0,
            })
        },
    )?;

    let mut stmt = c.prepare(
        "SELECT id, track_ms, session_id, session_offset_ms, note_text, rating, key, bpm, created_at, trim_start_ms, trim_end_ms, stem, source_mark_id
         FROM marks WHERE track_id = ?1 ORDER BY created_at ASC"
    )?;
    let mark_rows = stmt.query_map(params![track_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<i64>>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, Option<i64>>(9)?,
            r.get::<_, Option<i64>>(10)?,
            r.get::<_, Option<String>>(11)?,
            r.get::<_, Option<i64>>(12)?,
        ))
    })?;
    let mut marks = Vec::new();
    for r in mark_rows {
        let (id, track_ms, session_id, session_offset_ms, note_text, rating, key, bpm, created_at, trim_start_ms, trim_end_ms, stem, source_mark_id) = r?;
        let tags = tags_for(&c, id)?;
        marks.push(MarkDetail {
            id,
            track_ms,
            session_id,
            session_offset_ms,
            note_text,
            tags,
            rating,
            key,
            bpm,
            trim_start_ms,
            trim_end_ms,
            stem,
            source_mark_id,
            created_at,
        });
    }

    let mut stmt = c.prepare(
        "SELECT id, file_path, file_start_track_ms, sample_rate, channels
         FROM sessions WHERE track_id = ?1 ORDER BY created_at ASC"
    )?;
    let sess_rows = stmt.query_map(params![track_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut sessions = Vec::new();
    for r in sess_rows {
        let (id, file_path, file_start_track_ms, sample_rate, channels) = r?;
        let duration_ms = get_wav_duration_ms(&file_path);
        sessions.push(SessionRow {
            id,
            file_path,
            file_start_track_ms,
            sample_rate,
            channels,
            duration_ms,
        });
    }

    Ok(TrackDetail { track, marks, sessions })
}

pub fn get_session_info(s: &Store, session_id: i64) -> Result<(String, u16)> {
    let c = s.conn.lock().unwrap();
    let (file_path, channels) = c.query_row(
        "SELECT file_path, channels FROM sessions WHERE id = ?1",
        params![session_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u16)),
    )?;
    Ok((file_path, channels))
}

fn get_wav_duration_ms(path_str: &str) -> Option<i64> {
    let reader = hound::WavReader::open(path_str).ok()?;
    let spec = reader.spec();
    let frames = reader.duration() as f64;
    let sample_rate = spec.sample_rate as f64;
    Some(((frames / sample_rate) * 1000.0) as i64)
}

pub fn calculate_waveform(file_path: &str, channels: u16, buckets: u32) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(file_path)?;
    let total_samples = reader.duration() as usize;
    let ch = channels as usize;
    if ch == 0 {
        return Ok(vec![0.0; buckets as usize]);
    }
    let total_frames = total_samples;
    if total_frames == 0 || buckets == 0 {
        return Ok(vec![0.0; buckets as usize]);
    }

    let mut peaks = vec![0.0f32; buckets as usize];
    let mut samples_iter = reader.samples::<i16>();
    
    for f in 0..total_frames {
        let bucket_idx = ((f as u64 * buckets as u64) / total_frames as u64) as usize;
        let bucket_idx = bucket_idx.min(buckets as usize - 1);
        
        let mut frame_max = 0.0f32;
        for _ in 0..ch {
            if let Some(Ok(sample)) = samples_iter.next() {
                let val = (sample as f32 / 32768.0).abs();
                if val > frame_max {
                    frame_max = val;
                }
            }
        }
        
        if frame_max > peaks[bucket_idx] {
            peaks[bucket_idx] = frame_max;
        }
    }
    
    Ok(peaks)
}

pub fn get_track_by_id(s: &Store, track_id: i64) -> Result<TrackRow> {
    let c = s.conn.lock().unwrap();
    c.query_row(
        "SELECT id, spotify_id, title, artist, album, album_art_url, duration_ms, favorite
         FROM tracks WHERE id = ?1",
        params![track_id],
        |r| {
            Ok(TrackRow {
                id: r.get(0)?,
                spotify_id: r.get(1)?,
                title: r.get(2)?,
                artist: r.get(3)?,
                album: r.get(4)?,
                album_art_url: r.get(5)?,
                duration_ms: r.get(6)?,
                favorite: r.get::<_, i64>(7)? != 0,
            })
        },
    ).map_err(|e| e.into())
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct MarkExport {
    pub file_path: String,
    pub artist: String,
    pub title: String,
    pub track_ms: i64,
    pub tags: Vec<String>,
    pub trim_start_ms: Option<i64>,
    pub trim_end_ms: Option<i64>,
}

/// Gathers what's needed to export a mark's clip: the session WAV path, track
/// metadata, tags, and any trim region. Errors if the mark has no captured audio.
pub fn get_mark_for_export(s: &Store, mark_id: i64) -> Result<MarkExport> {
    let c = s.conn.lock().unwrap();
    let (file_path, artist, title, track_ms, trim_start_ms, trim_end_ms) = c.query_row(
        "SELECT se.file_path, t.artist, t.title, m.track_ms, m.trim_start_ms, m.trim_end_ms
         FROM marks m
         JOIN sessions se ON se.id = m.session_id
         JOIN tracks t ON t.id = m.track_id
         WHERE m.id = ?1",
        params![mark_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?)),
    ).map_err(|_| anyhow::anyhow!("this note has no captured audio to export"))?;
    let tags = tags_for(&c, mark_id)?;
    Ok(MarkExport { file_path, artist, title, track_ms, tags, trim_start_ms, trim_end_ms })
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct BridgeMark {
    pub id: i64,
    pub track_ms: i64,
    pub note_text: String,
    pub tags: Vec<String>,
}

/// Marks for a Spotify track id, for the Spicetify extension to render on the
/// seekbar. Ordered by position in the song.
pub fn marks_for_spotify(s: &Store, spotify_id: &str) -> Result<Vec<BridgeMark>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare(
        "SELECT m.id, m.track_ms, m.note_text
         FROM marks m JOIN tracks t ON t.id = m.track_id
         WHERE t.spotify_id = ?1 ORDER BY m.track_ms ASC")?;
    let rows: Vec<(i64, i64, String)> = stmt
        .query_map(params![spotify_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows.into_iter().map(|(id, track_ms, note_text)| {
        let tags = tags_for(&c, id).unwrap_or_default();
        BridgeMark { id, track_ms, note_text, tags }
    }).collect())
}

/// Sets or clears a mark's trim region (offsets in ms into its clip file).
pub fn update_mark_trim(s: &Store, mark_id: i64, trim_start_ms: Option<i64>, trim_end_ms: Option<i64>) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute(
        "UPDATE marks SET trim_start_ms = ?1, trim_end_ms = ?2, updated_at = ?3 WHERE id = ?4",
        params![trim_start_ms, trim_end_ms, now(), mark_id],
    )?;
    Ok(())
}

pub fn update_mark_session(s: &Store, mark_id: i64, session_id: i64, session_offset_ms: i64) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute(
        "UPDATE marks SET session_id = ?1, session_offset_ms = ?2 WHERE id = ?3",
        params![session_id, session_offset_ms, mark_id],
    )?;
    Ok(())
}


pub fn update_mark_meta(s: &Store, mark_id: i64, note_text: &str, tag_names: &[String],
    rating: Option<i64>, key: Option<String>, bpm: Option<i64>) -> Result<()> {
    let c = s.conn.lock().unwrap();
    let t = now();
    c.execute(
        "UPDATE marks SET note_text=?1, rating=?2, key=?3, bpm=?4, updated_at=?5 WHERE id=?6",
        params![note_text, rating, key, bpm, t, mark_id],
    )?;
    c.execute("DELETE FROM mark_tags WHERE mark_id=?1", params![mark_id])?;
    for name in tag_names {
        let tag_id: i64 = c.query_row("SELECT id FROM tags WHERE name=?1", params![name], |r| r.get(0))?;
        c.execute("INSERT OR IGNORE INTO mark_tags(mark_id, tag_id) VALUES (?1,?2)", params![mark_id, tag_id])?;
    }
    Ok(())
}

pub fn delete_mark(s: &Store, mark_id: i64) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute("DELETE FROM marks WHERE id = ?1", params![mark_id])?;
    Ok(())
}

/// One separated stem ready to be recorded in the library.
pub struct StemInsert {
    pub file_path: String,
    /// One of `stems::separator::STEM_NAMES`.
    pub stem: String,
    pub sample_rate: i64,
    pub channels: i64,
}

/// Tag each stem gets, mapped onto the locked palette (defined in `src/css/tokens.css` —
/// the tag set and colours are fixed, so stems reuse existing tags rather than adding any).
fn tag_for_stem(stem: &str) -> &'static str {
    match stem {
        "drums" => "break",
        "bass" => "bass",
        "vocals" => "vocal",
        _ => "texture",
    }
}

/// Record four separated stems as ordinary library notes: one `sessions` row per
/// stem WAV plus one `marks` row on it, carrying provenance back to the mark they
/// were split from.
///
/// All-or-nothing: a failure part-way leaves the library exactly as it was, so a
/// half-written split can never appear as two orphan stems.
pub fn insert_stem_marks(s: &Store, source_mark_id: i64, stems: &[StemInsert]) -> Result<Vec<i64>> {
    let mut c = s.conn.lock().unwrap();
    // Where the separated region actually sits in the song: the capture's start plus
    // the note's in-point. Using the source mark's track_ms put every stem at the
    // capture's start instead, so a trimmed note's stems reported the wrong time.
    let (track_id, note_text, sess_start, trim_start): (i64, String, Option<i64>, Option<i64>) = c
        .query_row(
            "SELECT m.track_id, m.note_text, se.file_start_track_ms, m.trim_start_ms
             FROM marks m LEFT JOIN sessions se ON se.id = m.session_id
             WHERE m.id = ?1",
            params![source_mark_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| anyhow::anyhow!("unknown source mark {source_mark_id}"))?;
    let track_ms = sess_start.unwrap_or(0) + trim_start.unwrap_or(0);

    let tx = c.transaction()?;
    let t = now();
    let mut ids = Vec::with_capacity(stems.len());
    for st in stems {
        tx.execute(
            "INSERT INTO sessions(track_id,file_path,file_start_track_ms,sample_rate,channels,created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![track_id, st.file_path, track_ms, st.sample_rate, st.channels, t],
        )?;
        let session_id = tx.last_insert_rowid();

        let label = if note_text.trim().is_empty() {
            st.stem.clone()
        } else {
            format!("{} — {}", note_text.trim(), st.stem)
        };
        tx.execute(
            "INSERT INTO marks(track_id,session_id,track_ms,session_offset_ms,note_text,
                               source_mark_id,stem,created_at,updated_at)
             VALUES (?1,?2,?3,0,?4,?5,?6,?7,?7)",
            params![track_id, session_id, track_ms, label, source_mark_id, st.stem, t],
        )?;
        let mark_id = tx.last_insert_rowid();

        let tag = tag_for_stem(&st.stem);
        if let Ok(tag_id) = tx.query_row::<i64, _, _>(
            "SELECT id FROM tags WHERE name=?1",
            params![tag],
            |r| r.get(0),
        ) {
            tx.execute(
                "INSERT OR IGNORE INTO mark_tags(mark_id, tag_id) VALUES (?1,?2)",
                params![mark_id, tag_id],
            )?;
        }
        ids.push(mark_id);
    }
    tx.commit()?;
    Ok(ids)
}

/// Stem marks already produced from this mark, so the UI can offer "4 stems"
/// instead of splitting again by accident.
pub fn stem_marks_for(s: &Store, source_mark_id: i64) -> Result<Vec<i64>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare("SELECT id FROM marks WHERE source_mark_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![source_mark_id], |r| r.get::<_, i64>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn set_favorite(s: &Store, track_id: i64, favorite: bool) -> Result<()> {
    let c = s.conn.lock().unwrap();
    c.execute("UPDATE tracks SET favorite = ?1 WHERE id = ?2",
        params![favorite as i64, track_id])?;
    Ok(())
}

/// Deletes a whole track: its marks (mark_tags cascade), its sessions, and the
/// track row. Returns the session file paths so the caller can delete the WAVs.
pub fn delete_track(s: &Store, track_id: i64) -> Result<Vec<String>> {
    let c = s.conn.lock().unwrap();
    let mut stmt = c.prepare("SELECT file_path FROM sessions WHERE track_id = ?1")?;
    let files: Vec<String> = stmt.query_map(params![track_id], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok()).collect();
    drop(stmt);
    c.execute("DELETE FROM marks WHERE track_id = ?1", params![track_id])?;
    c.execute("DELETE FROM sessions WHERE track_id = ?1", params![track_id])?;
    c.execute("DELETE FROM tracks WHERE id = ?1", params![track_id])?;
    Ok(files)
}

/// One stem of a split, as the mixer needs it.
#[derive(serde::Serialize, Clone, Debug)]
pub struct StemRow {
    pub mark_id: i64,
    pub session_id: i64,
    /// One of `stems::separator::STEM_NAMES`.
    pub stem: String,
    pub file_path: String,
    pub sample_rate: Option<i64>,
    pub duration_ms: Option<i64>,
}

/// The four stems split from one note, plus what the mixer shows about it.
#[derive(serde::Serialize, Clone, Debug)]
pub struct StemSet {
    pub source_mark_id: i64,
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub note_text: String,
    /// Where the separated region sits in the song.
    pub track_ms: i64,
    pub bpm: Option<i64>,
    pub key: Option<String>,
    pub stems: Vec<StemRow>,
}

/// Marks with one of these `stem` values are the raw separator output. A saved
/// mix carries `stem = "mix"` and behaves as an ordinary note.
pub const RAW_STEMS: [&str; 4] = ["drums", "bass", "other", "vocals"];

/// Resolve the stem set a mark belongs to — whether `mark_id` is the note that
/// was split or one of the stems it produced. `None` when there is no split.
/// If a note was split more than once, the newest stem of each kind wins.
pub fn get_stem_set(s: &Store, mark_id: i64) -> Result<Option<StemSet>> {
    let c = s.conn.lock().unwrap();
    let source_id: i64 = c
        .query_row(
            "SELECT CASE WHEN stem IS NOT NULL AND stem != 'mix' AND source_mark_id IS NOT NULL
                         THEN source_mark_id ELSE id END
             FROM marks WHERE id = ?1",
            params![mark_id],
            |r| r.get(0),
        )
        .map_err(|_| anyhow::anyhow!("unknown mark {mark_id}"))?;

    let mut stmt = c.prepare(
        "SELECT m.id, m.session_id, m.stem, se.file_path, se.sample_rate
         FROM marks m JOIN sessions se ON se.id = m.session_id
         WHERE m.source_mark_id = ?1 AND m.stem IN ('drums','bass','other','vocals')
         ORDER BY m.id DESC",
    )?;
    let rows = stmt.query_map(params![source_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut by_stem: std::collections::HashMap<String, StemRow> = std::collections::HashMap::new();
    for r in rows {
        let (id, session_id, stem, file_path, sample_rate) = r?;
        by_stem.entry(stem.clone()).or_insert_with(|| StemRow {
            mark_id: id,
            session_id,
            stem,
            duration_ms: get_wav_duration_ms(&file_path),
            file_path,
            sample_rate,
        });
    }
    if by_stem.is_empty() {
        return Ok(None);
    }
    let stems: Vec<StemRow> = RAW_STEMS
        .iter()
        .filter_map(|name| by_stem.remove(*name))
        .collect();

    let (track_id, title, artist, note_text, sess_start, trim_start, bpm, key) = c
        .query_row(
            "SELECT m.track_id, t.title, t.artist, m.note_text, se.file_start_track_ms, m.trim_start_ms, m.bpm, m.key
             FROM marks m JOIN tracks t ON t.id = m.track_id
             LEFT JOIN sessions se ON se.id = m.session_id
             WHERE m.id = ?1",
            params![source_id],
            |r| Ok((
                r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                r.get::<_, String>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?, r.get::<_, Option<String>>(7)?,
            )),
        )
        .map_err(|_| anyhow::anyhow!("the note these stems came from no longer exists"))?;
    Ok(Some(StemSet {
        source_mark_id: source_id,
        track_id,
        title,
        artist,
        note_text,
        track_ms: sess_start.unwrap_or(0) + trim_start.unwrap_or(0),
        bpm,
        key,
        stems,
    }))
}

/// Record a mix the owner bounced from the mixer as an ordinary note on the same
/// track, tagged with every stem that is audible in it and pointing back at the
/// note it was split from. Returns the new mark id.
pub fn insert_mix_mark(
    s: &Store,
    source_mark_id: i64,
    file_path: &str,
    included: &[&str],
    sample_rate: i64,
    channels: i64,
) -> Result<i64> {
    let mut c = s.conn.lock().unwrap();
    let (track_id, note_text, sess_start, trim_start): (i64, String, Option<i64>, Option<i64>) = c
        .query_row(
            "SELECT m.track_id, m.note_text, se.file_start_track_ms, m.trim_start_ms
             FROM marks m LEFT JOIN sessions se ON se.id = m.session_id
             WHERE m.id = ?1",
            params![source_mark_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| anyhow::anyhow!("unknown source mark {source_mark_id}"))?;
    let track_ms = sess_start.unwrap_or(0) + trim_start.unwrap_or(0);
    let what = if included.len() == RAW_STEMS.len() { "mix".to_string() }
               else { format!("mix ({})", included.join(" + ")) };
    let label = if note_text.trim().is_empty() { what } else { format!("{} — {}", note_text.trim(), what) };

    let tx = c.transaction()?;
    let t = now();
    tx.execute(
        "INSERT INTO sessions(track_id,file_path,file_start_track_ms,sample_rate,channels,created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![track_id, file_path, track_ms, sample_rate, channels, t],
    )?;
    let session_id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO marks(track_id,session_id,track_ms,session_offset_ms,note_text,
                           source_mark_id,stem,created_at,updated_at)
         VALUES (?1,?2,?3,0,?4,?5,'mix',?6,?6)",
        params![track_id, session_id, track_ms, label, source_mark_id, t],
    )?;
    let mark_id = tx.last_insert_rowid();
    let mut tags: Vec<&str> = included.iter().map(|st| tag_for_stem(st)).collect();
    tags.sort();
    tags.dedup();
    for tag in tags {
        if let Ok(tag_id) = tx.query_row::<i64, _, _>("SELECT id FROM tags WHERE name=?1", params![tag], |r| r.get(0)) {
            tx.execute("INSERT OR IGNORE INTO mark_tags(mark_id, tag_id) VALUES (?1,?2)", params![mark_id, tag_id])?;
        }
    }
    tx.commit()?;
    Ok(mark_id)
}

/// Remove every raw stem split from `source_mark_id` — marks and their sessions.
/// Saved mixes are ordinary notes and are left alone. Returns the stem WAV paths
/// so the caller can delete the files; the source capture is never touched.
pub fn delete_stem_set(s: &Store, source_mark_id: i64) -> Result<Vec<String>> {
    let mut c = s.conn.lock().unwrap();
    let tx = c.transaction()?;
    let mut files = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT m.id, m.session_id, se.file_path
             FROM marks m JOIN sessions se ON se.id = m.session_id
             WHERE m.source_mark_id = ?1 AND m.stem IN ('drums','bass','other','vocals')",
        )?;
        let rows: Vec<(i64, i64, String)> = stmt
            .query_map(params![source_mark_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        for (mark_id, session_id, path) in rows {
            tx.execute("DELETE FROM marks WHERE id = ?1", params![mark_id])?;
            tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
            files.push(path);
        }
    }
    tx.commit()?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Store {
        Store::open_in_memory().unwrap()
    }

    /// A real, silent WAV on disk — needed by tests that exercise duration clamping.
    fn wav_fixture(dir: &tempfile::TempDir, name: &str, seconds: f64, sr: u32) -> String {
        let path = dir.path().join(name);
        let frames = (seconds * sr as f64) as usize;
        let samples = vec![0.0f32; frames * 2]; // stereo
        crate::session::write_wav_i16(&path, &samples, sr, 2).unwrap();
        path.to_string_lossy().to_string()
    }

    fn stem(path: &str, name: &str) -> StemInsert {
        StemInsert { file_path: path.into(), stem: name.into(), sample_rate: 48000, channels: 2 }
    }

    #[test]
    fn insert_stem_marks_creates_four_sessions_and_marks_with_provenance() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:stem", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/src.wav", 60000, 48000, 2).unwrap();
        let src = insert_mark(&s, track_id, 62000, Some(sess), Some(2000), "big break").unwrap();

        let ids = insert_stem_marks(&s, src, &[
            stem("C:/clips/d.wav", "drums"), stem("C:/clips/b.wav", "bass"),
            stem("C:/clips/o.wav", "other"), stem("C:/clips/v.wav", "vocals"),
        ]).unwrap();

        assert_eq!(ids.len(), 4);
        let c = s.conn.lock().unwrap();
        // Four NEW sessions on top of the original capture.
        let sessions: i64 = c.query_row("SELECT COUNT(*) FROM sessions WHERE track_id=?1", params![track_id], |r| r.get(0)).unwrap();
        assert_eq!(sessions, 5, "one per stem plus the original capture");

        for (id, name) in ids.iter().zip(["drums", "bass", "other", "vocals"]) {
            let (srcm, st, note, tms): (i64, String, String, i64) = c.query_row(
                "SELECT source_mark_id, stem, note_text, track_ms FROM marks WHERE id=?1",
                params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
            assert_eq!(srcm, src, "stem must point back at the mark it came from");
            assert_eq!(st, name);
            assert_eq!(note, format!("big break — {name}"));
            // Untrimmed source: the whole capture was separated, so the stems start
            // where the capture starts (60000), not at the note's track_ms.
            assert_eq!(tms, 60000, "stem sits where the separated region starts");
        }
        // Tags come from the locked palette.
        assert_eq!(tags_for(&c, ids[0]).unwrap(), vec!["break"]);
        assert_eq!(tags_for(&c, ids[3]).unwrap(), vec!["vocal"]);
    }

    #[test]
    fn stems_of_a_trimmed_note_land_at_the_regions_song_position() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:tstem", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/src.wav", 60000, 48000, 2).unwrap();
        // Only the region 5s-9s into the capture is separated.
        let src = create_mark_from_selection(&s, sess, 5000, 9000, "loop", &[]).unwrap();

        let ids = insert_stem_marks(&s, src, &[stem("C:/clips/d.wav", "drums")]).unwrap();

        let c = s.conn.lock().unwrap();
        let tms: i64 = c.query_row("SELECT track_ms FROM marks WHERE id=?1", params![ids[0]], |r| r.get(0)).unwrap();
        assert_eq!(tms, 65000, "capture start 60000 + the note's 5000 in-point");
    }

    #[test]
    fn insert_stem_marks_is_all_or_nothing() {
        let s = mem();
        // Unknown source mark: nothing may be written at all.
        let before: i64 = { let c = s.conn.lock().unwrap();
            c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap() };
        assert!(insert_stem_marks(&s, 9999, &[stem("C:/x.wav", "drums")]).is_err());
        let after: i64 = { let c = s.conn.lock().unwrap();
            c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap() };
        assert_eq!(before, after, "a failed split must leave the library untouched");
    }

    #[test]
    fn stem_marks_for_reports_existing_splits() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:sm", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/src.wav", 0, 48000, 2).unwrap();
        let src = insert_mark(&s, track_id, 0, Some(sess), Some(0), "n").unwrap();
        assert!(stem_marks_for(&s, src).unwrap().is_empty(), "no stems before splitting");
        let ids = insert_stem_marks(&s, src, &[stem("C:/d.wav", "drums")]).unwrap();
        assert_eq!(stem_marks_for(&s, src).unwrap(), ids);
    }

    #[test]
    fn create_mark_from_selection_computes_track_ms_from_session_start() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:sel", "T", "A", "Al", None, 240000).unwrap();
        // Capture begins 2:40 into the song.
        let sess = insert_session(&s, track_id, "C:/clips/sel.wav", 160000, 48000, 2).unwrap();

        let id = create_mark_from_selection(&s, sess, 5000, 9000, "second loop", &[]).unwrap();

        let c = s.conn.lock().unwrap();
        let (track_ms, off, ts, te, sid): (i64, i64, i64, i64, i64) = c.query_row(
            "SELECT track_ms, session_offset_ms, trim_start_ms, trim_end_ms, session_id FROM marks WHERE id=?1",
            params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap();
        assert_eq!(track_ms, 165000, "track_ms = file_start_track_ms + trim_start_ms");
        assert_eq!(off, 5000);
        assert_eq!(ts, 5000);
        assert_eq!(te, 9000);
        assert_eq!(sid, sess, "new mark points at the SAME capture, no new session");
    }

    #[test]
    fn create_mark_from_selection_rejects_unknown_session() {
        let s = mem();
        let err = create_mark_from_selection(&s, 4242, 0, 1000, "nope", &[]).unwrap_err();
        assert!(err.to_string().contains("unknown capture session"), "got: {err}");
    }

    #[test]
    fn create_mark_from_selection_rejects_inverted_range() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:inv", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/inv.wav", 0, 48000, 2).unwrap();

        assert!(create_mark_from_selection(&s, sess, 9000, 9000, "", &[]).is_err(), "zero length");
        assert!(create_mark_from_selection(&s, sess, 9000, 4000, "", &[]).is_err(), "inverted");
        assert!(create_mark_from_selection(&s, sess, -1, 4000, "", &[]).is_err(), "negative start");

        // Nothing was inserted by any of the three rejections.
        let c = s.conn.lock().unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM marks", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn create_mark_from_selection_clamps_to_session_duration() {
        let s = mem();
        let dir = tempfile::tempdir().unwrap();
        let wav = wav_fixture(&dir, "clamp.wav", 4.0, 48000); // 4000 ms
        let track_id = upsert_track(&s, "spotify:track:clamp", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, &wav, 0, 48000, 2).unwrap();

        let id = create_mark_from_selection(&s, sess, 1000, 99999, "past the end", &[]).unwrap();

        let c = s.conn.lock().unwrap();
        let te: i64 = c.query_row("SELECT trim_end_ms FROM marks WHERE id=?1", params![id], |r| r.get(0)).unwrap();
        assert!((te - 4000).abs() <= 1, "clamped to the WAV's real length, got {te}");
    }

    #[test]
    fn create_mark_from_selection_links_tags() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:tag", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/tag.wav", 0, 48000, 2).unwrap();

        let id = create_mark_from_selection(
            &s, sess, 0, 2000, "tagged loop", &["break".to_string(), "bass".to_string()]).unwrap();

        let c = s.conn.lock().unwrap();
        let tags = tags_for(&c, id).unwrap();
        assert_eq!(tags.len(), 2);
        assert!(tags.contains(&"break".to_string()));
        assert!(tags.contains(&"bass".to_string()));
    }

    #[test]
    fn store_crud_roundtrip() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:123", "Title", "Artist", "Album", Some("http://url"), 240000).unwrap();
        let mark_id = insert_mark(&s, track_id, 120000, None, None, "Vocal chop").unwrap();
        set_mark_text_tags(&s, mark_id, "Vocal chop", &["vocal".to_string()]).unwrap();
        let marks = list_marks(&s).unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].note_text, "Vocal chop");
        assert_eq!(marks[0].tags, vec!["vocal"]);
    }

    #[test]
    fn get_marks_by_ids_preserves_order_and_skips_missing() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:pg", "PG Title", "PG Artist", "Album", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/pg.wav", 30000, 48000, 2).unwrap();
        let m1 = insert_mark(&s, track_id, 32000, Some(sess), Some(2000), "first").unwrap();
        let m2 = insert_mark(&s, track_id, 40000, Some(sess), Some(10000), "second").unwrap();
        set_mark_text_tags(&s, m1, "first", &["break".to_string()]).unwrap();
        // A mark with no session must be skipped (nothing to play).
        let orphan = insert_mark(&s, track_id, 50000, None, None, "orphan").unwrap();

        let clips = get_marks_by_ids(&s, &[m2, 99999, m1, orphan]).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].mark_id, m2);
        assert_eq!(clips[0].note_text, "second");
        assert_eq!(clips[1].mark_id, m1);
        assert_eq!(clips[1].title, "PG Title");
        assert_eq!(clips[1].file_path, "C:/clips/pg.wav");
        assert_eq!(clips[1].tags, vec!["break"]);
    }

    #[test]
    fn fts_finds_mark_by_note_text() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:123", "Title", "Artist", "Album", Some("http://url"), 240000).unwrap();
        let mark_id = insert_mark(&s, track_id, 120000, None, None, "Amazing vocal hook").unwrap();
        let hits = search_marks(&s, "vocal").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, mark_id);
    }

    #[test]
    fn settings_roundtrip() {
        let s = mem();
        assert_eq!(get_setting(&s, "pre_roll").unwrap(), None);
        set_setting(&s, "pre_roll", "30").unwrap();
        assert_eq!(get_setting(&s, "pre_roll").unwrap(), Some("30".into()));
    }

    #[test]
    fn seed_tags_present() {
        let s = mem();
        let names = tag_names(&s).unwrap();
        for t in ["break", "vocal", "melody", "bass", "chords", "texture", "fx"] {
            assert!(names.contains(&t.to_string()), "missing seed tag {t}");
        }
    }

    #[test]
    fn session_insert_and_latest() {
        let s = mem();
        let t = upsert_track(&s, "sp1", "A", "B", "C", None, 1000).unwrap();
        let sid = insert_session(&s, t, "x.wav", 0, 48000, 2).unwrap();
        assert_eq!(latest_session_for_track(&s, t).unwrap(), Some(sid));
    }

    #[test]
    fn favorite_defaults_false_and_toggles() {
        let s = mem();
        let t = upsert_track(&s, "sp1", "A", "B", "C", None, 1000).unwrap();
        insert_mark(&s, t, 0, None, None, "n").unwrap();
        assert!(!get_library(&s).unwrap()[0].favorite);
        set_favorite(&s, t, true).unwrap();
        assert!(get_library(&s).unwrap()[0].favorite);
        assert!(get_track_by_id(&s, t).unwrap().favorite);
        set_favorite(&s, t, false).unwrap();
        assert!(!get_track_by_id(&s, t).unwrap().favorite);
    }

    #[test]
    fn mark_export_info_requires_audio() {
        let s = mem();
        let t = upsert_track(&s, "sp1", "Halo", "Catching Flies", "Tides", None, 1000).unwrap();
        let no_audio = insert_mark(&s, t, 5000, None, None, "n").unwrap();
        assert!(get_mark_for_export(&s, no_audio).is_err());
        let sid = insert_session(&s, t, "clip.wav", 0, 48000, 2).unwrap();
        let m = insert_mark(&s, t, 83000, Some(sid), Some(0), "n").unwrap();
        set_mark_text_tags(&s, m, "n", &["vocal".to_string()]).unwrap();
        let e = get_mark_for_export(&s, m).unwrap();
        assert_eq!(e.file_path, "clip.wav");
        assert_eq!(e.title, "Halo");
        assert_eq!(e.track_ms, 83000);
        assert_eq!(e.tags, vec!["vocal"]);
    }

    #[test]
    fn delete_track_removes_rows_and_returns_files() {
        let s = mem();
        let t = upsert_track(&s, "sp1", "A", "B", "C", None, 1000).unwrap();
        let sid = insert_session(&s, t, "clip.wav", 0, 48000, 2).unwrap();
        insert_mark(&s, t, 0, Some(sid), Some(0), "n").unwrap();
        let files = delete_track(&s, t).unwrap();
        assert_eq!(files, vec!["clip.wav".to_string()]);
        assert_eq!(get_library(&s).unwrap().len(), 0);
        assert!(list_marks(&s).unwrap().is_empty());
    }

    #[test]
    fn test_waveform_peak() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_spike.wav");
        
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..600 {
            if i == 300 {
                writer.write_sample(32767i16).unwrap(); // loud spike
            } else {
                writer.write_sample(0i16).unwrap();
            }
        }
        writer.finalize().unwrap();
        
        let peaks = calculate_waveform(path.to_str().unwrap(), 1, 6).unwrap();
        assert_eq!(peaks.len(), 6);
        assert_eq!(peaks[0], 0.0);
        assert_eq!(peaks[1], 0.0);
        assert_eq!(peaks[2], 0.0);
        assert!(peaks[3] > 0.99);
        assert_eq!(peaks[4], 0.0);
        assert_eq!(peaks[5], 0.0);

        // Test stereo: 2 channels, 1000 frames total. Spike in frame 500.
        let path2 = dir.path().join("test_stereo_spike.wav");
        let spec2 = hound::WavSpec {
            channels: 2,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer2 = hound::WavWriter::create(&path2, spec2).unwrap();
        for f in 0..1000 {
            if f == 500 {
                writer2.write_sample(0i16).unwrap(); // L
                writer2.write_sample(32767i16).unwrap(); // R (spike)
            } else {
                writer2.write_sample(0i16).unwrap();
                writer2.write_sample(0i16).unwrap();
            }
        }
        writer2.finalize().unwrap();

        let peaks2 = calculate_waveform(path2.to_str().unwrap(), 2, 5).unwrap();
        assert_eq!(peaks2.len(), 5);
        assert_eq!(peaks2[0], 0.0);
        assert_eq!(peaks2[1], 0.0);
        assert!(peaks2[2] > 0.99); // middle bucket (frame 500 falls in bucket 2)
        assert_eq!(peaks2[3], 0.0);
        assert_eq!(peaks2[4], 0.0);
    }

    #[test]
    fn test_get_library_query() {
        let s = mem();
        let t1 = upsert_track(&s, "sp1", "Track A", "Artist X", "Album", None, 1000).unwrap();
        let t2 = upsert_track(&s, "sp2", "Track B", "Artist Y", "Album", None, 1000).unwrap();
        
        let _m1 = insert_mark(&s, t1, 100, None, None, "Note A").unwrap();
        let _m2 = insert_mark(&s, t2, 200, None, None, "Note B").unwrap();
        
        let lib = get_library(&s).unwrap();
        assert_eq!(lib.len(), 2);
        assert_eq!(lib[0].track_id, t2);
        assert_eq!(lib[1].track_id, t1);
    }

    #[test]
    fn test_update_mark_meta_roundtrip() {
        let s = mem();
        let t = upsert_track(&s, "sp1", "Track A", "Artist X", "Album", None, 1000).unwrap();
        let m = insert_mark(&s, t, 100, None, None, "Old Note").unwrap();
        
        update_mark_meta(&s, m, "New Note", &["vocal".to_string()], Some(5), Some("C#m".to_string()), Some(120)).unwrap();
        
        let detail = get_track_detail(&s, t).unwrap();
        assert_eq!(detail.marks.len(), 1);
        assert_eq!(detail.marks[0].note_text, "New Note");
        assert_eq!(detail.marks[0].tags, vec!["vocal".to_string()]);
        assert_eq!(detail.marks[0].rating, Some(5));
        assert_eq!(detail.marks[0].key.as_deref(), Some("C#m"));
        assert_eq!(detail.marks[0].bpm, Some(120));
    }

    #[test]
    fn test_delete_mark_db() {
        let s = mem();
        let t = upsert_track(&s, "sp1", "Track A", "Artist X", "Album", None, 1000).unwrap();
        let m = insert_mark(&s, t, 100, None, None, "Note").unwrap();
        
        set_mark_text_tags(&s, m, "Note", &["vocal".to_string()]).unwrap();
        
        let detail = get_track_detail(&s, t).unwrap();
        assert_eq!(detail.marks.len(), 1);
        
        delete_mark(&s, m).unwrap();
        
        let detail = get_track_detail(&s, t).unwrap();
        assert_eq!(detail.marks.len(), 0);
    }

    #[test]
    fn test_stereo_wav_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo_3s.wav");
        
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..288000 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        
        let dur = get_wav_duration_ms(path.to_str().unwrap());
        assert_eq!(dur, Some(3000));
    }

    #[test]
    fn get_stem_set_resolves_from_source_or_stem_and_is_none_for_plain_notes() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:set", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/src.wav", 60000, 48000, 2).unwrap();
        let src = insert_mark(&s, track_id, 60000, Some(sess), Some(0), "loop").unwrap();
        update_mark_meta(&s, src, "loop", &[], None, Some("Em".to_string()), Some(120)).unwrap();
        assert!(get_stem_set(&s, src).unwrap().is_none(), "no split yet");

        let ids = insert_stem_marks(&s, src, &[
            stem("C:/clips/d.wav", "drums"), stem("C:/clips/b.wav", "bass"),
            stem("C:/clips/o.wav", "other"), stem("C:/clips/v.wav", "vocals"),
        ]).unwrap();

        let from_source = get_stem_set(&s, src).unwrap().expect("set from the source note");
        let from_stem = get_stem_set(&s, ids[2]).unwrap().expect("set from one of its stems");
        assert_eq!(from_source.source_mark_id, src);
        assert_eq!(from_stem.source_mark_id, src, "a stem resolves back to its source");
        let names: Vec<&str> = from_source.stems.iter().map(|r| r.stem.as_str()).collect();
        assert_eq!(names, RAW_STEMS, "stems come back in the fixed drums/bass/other/vocals order");
        assert_eq!(from_source.stems[1].file_path, "C:/clips/b.wav");
        assert_eq!(from_source.track_ms, 60000);
        assert_eq!(from_source.title, "T");
        assert_eq!(from_source.bpm, Some(120));
        assert_eq!(from_source.key, Some("Em".to_string()));
        assert_eq!(from_stem.bpm, Some(120));
        assert_eq!(from_stem.key, Some("Em".to_string()));
    }

    #[test]
    fn get_stem_set_prefers_the_newest_split_of_each_stem() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:re", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/src.wav", 0, 48000, 2).unwrap();
        let src = insert_mark(&s, track_id, 0, Some(sess), Some(0), "").unwrap();
        insert_stem_marks(&s, src, &[stem("C:/old/d.wav", "drums")]).unwrap();
        insert_stem_marks(&s, src, &[stem("C:/new/d.wav", "drums")]).unwrap();
        let set = get_stem_set(&s, src).unwrap().unwrap();
        assert_eq!(set.stems.len(), 1);
        assert_eq!(set.stems[0].file_path, "C:/new/d.wav");
    }

    #[test]
    fn insert_mix_mark_is_an_ordinary_note_with_provenance_and_stem_tags() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:mix", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/src.wav", 30000, 48000, 2).unwrap();
        let src = insert_mark(&s, track_id, 30000, Some(sess), Some(0), "the drop").unwrap();
        update_mark_trim(&s, src, Some(5000), Some(9000)).unwrap();

        let id = insert_mix_mark(&s, src, "C:/clips/mix.wav", &["drums", "bass"], 48000, 2).unwrap();
        let detail = get_track_detail(&s, track_id).unwrap();
        let m = detail.marks.iter().find(|m| m.id == id).unwrap();
        assert_eq!(m.stem.as_deref(), Some("mix"));
        assert_eq!(m.source_mark_id, Some(src));
        assert_eq!(m.note_text, "the drop — mix (drums + bass)");
        assert_eq!(m.track_ms, 35000, "mix sits where the trimmed region starts");
        let mut tags = m.tags.clone();
        tags.sort();
        assert_eq!(tags, vec!["bass", "break"]);
        assert!(get_stem_set(&s, src).unwrap().is_none(), "a mix is not a raw stem");

        let all = insert_mix_mark(&s, src, "C:/clips/mix2.wav", &RAW_STEMS, 48000, 2).unwrap();
        let detail = get_track_detail(&s, track_id).unwrap();
        assert_eq!(detail.marks.iter().find(|m| m.id == all).unwrap().note_text, "the drop — mix");
    }

    #[test]
    fn delete_stem_set_removes_stems_and_sessions_but_keeps_source_and_mixes() {
        let s = mem();
        let track_id = upsert_track(&s, "spotify:track:del", "T", "A", "Al", None, 240000).unwrap();
        let sess = insert_session(&s, track_id, "C:/clips/src.wav", 0, 48000, 2).unwrap();
        let src = insert_mark(&s, track_id, 0, Some(sess), Some(0), "").unwrap();
        insert_stem_marks(&s, src, &[
            stem("C:/clips/d.wav", "drums"), stem("C:/clips/b.wav", "bass"),
            stem("C:/clips/o.wav", "other"), stem("C:/clips/v.wav", "vocals"),
        ]).unwrap();
        let mix = insert_mix_mark(&s, src, "C:/clips/mix.wav", &["drums"], 48000, 2).unwrap();

        let mut files = delete_stem_set(&s, src).unwrap();
        files.sort();
        assert_eq!(files, vec!["C:/clips/b.wav", "C:/clips/d.wav", "C:/clips/o.wav", "C:/clips/v.wav"]);
        assert!(get_stem_set(&s, src).unwrap().is_none());
        let detail = get_track_detail(&s, track_id).unwrap();
        let ids: Vec<i64> = detail.marks.iter().map(|m| m.id).collect();
        assert!(ids.contains(&src), "the source note survives");
        assert!(ids.contains(&mix), "a saved mix survives");
        assert_eq!(detail.sessions.len(), 2, "source capture + mix; stem sessions are gone");
    }
}
