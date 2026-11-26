use crate::capture::RingBuffer;
use crate::clock::Clock;
use crate::spotify::Tokens;
use crate::store::{self, Store};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct Config {
    pub pre_roll: f64,
    pub post_roll: f64,
    pub client_id: String,
    pub clips_dir: PathBuf,
    pub capture_hotkey: String,
    pub buffer_seconds: f64,
    pub system_capture: bool,
}

impl Config {
    pub fn load(s: &Store) -> Config {
        let get = |k: &str| store::get_setting(s, k).ok().flatten();
        let num = |k: &str, d: f64| get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let clips_dir = get("clips_dir").map(PathBuf::from).unwrap_or_else(|| {
            dirs::document_dir().unwrap_or_default().join("Samplify").join("Clips")
        });
        let capture_hotkey = get("capture_hotkey")
            .or_else(|| get("hotkey_popup")) // migrate: reuse popup-hotkey
            .unwrap_or_else(|| "CommandOrControl+Shift+N".into());
        Config {
            pre_roll: num("pre_roll", 30.0),
            post_roll: num("post_roll", 15.0),
            // Default to the project's Spotify app (public PKCE client id — not a
            // secret). Users authorized on that app's allowlist just click Connect;
            // a stored non-empty value still overrides this.
            client_id: get("client_id").filter(|s| !s.is_empty())
                .unwrap_or_else(|| "0e234b21409f412486001fec84b9314e".to_string()),
            clips_dir,
            capture_hotkey,
            buffer_seconds: num("buffer_seconds", 45.0).clamp(30.0, 300.0),
            system_capture: get("capture_source").as_deref() == Some("system"),
        }
    }
}

pub struct AppState {
    #[cfg(windows)]
    pub desktop_audio: crate::capture_win::WindowsAudio,
    pub store: Store,
    pub clock: Mutex<Clock>,
    pub ring: Arc<Mutex<RingBuffer>>,
    pub tokens: Mutex<Option<Tokens>>,
    pub config: Mutex<Config>,
    /// Latest track metadata pushed by the Spicetify extension. Used as the
    /// capture metadata source (Spotify knows the track directly), falling back
    /// to the Web API only when the extension isn't connected.
    pub bridge_track: Mutex<Option<(std::time::Instant, crate::spotify::NowPlaying)>>,
    /// True while an OAuth login is running. A second concurrent login would fight
    /// over the loopback callback port and cross its CSRF state — so we reject it.
    pub auth_in_progress: std::sync::atomic::AtomicBool,
}

// The extension publishes every 500 ms, including when playback is paused.
fn recent_player_update(received: std::time::Instant, now: std::time::Instant) -> bool {
    now.duration_since(received) <= std::time::Duration::from_secs(5)
}

impl AppState {
    pub fn spotify_player_track(&self) -> Option<crate::spotify::NowPlaying> {
        self.bridge_track.lock().unwrap().as_ref()
            .filter(|(received, _)| recent_player_update(*received, std::time::Instant::now()))
            .map(|(_, track)| track.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn spotify_metadata_expires_after_missing_player_updates() {
        let tick = std::time::Instant::now();
        assert!(recent_player_update(tick, tick + std::time::Duration::from_secs(2)));
        assert!(!recent_player_update(tick, tick + std::time::Duration::from_secs(6)));
    }

    #[test]
    fn config_defaults_when_unset() {
        let s = Store::open_in_memory().unwrap();
        let c = Config::load(&s);
        assert_eq!(c.capture_hotkey, "CommandOrControl+Shift+N");
        assert_eq!(c.buffer_seconds, 45.0);
    }

    #[test]
    fn config_reads_overrides() {
        let s = Store::open_in_memory().unwrap();
        crate::store::set_setting(&s, "buffer_seconds", "45").unwrap();
        let c = Config::load(&s);
        assert_eq!(c.buffer_seconds, 45.0);
    }
}
