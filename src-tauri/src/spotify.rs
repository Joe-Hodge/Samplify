use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::io::{Read, Write};
use std::net::TcpListener;

const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

pub const REDIRECT_URI: &str = "http://127.0.0.1:5723/callback";
pub const SCOPES: &str = "user-read-playback-state user-read-currently-playing user-modify-playback-state";

/// Pauses playback on the active device. Best-effort: returns Ok even on 403/404
/// (no modify scope yet, or no active device) so callers can ignore the result.
pub async fn pause(access_token: &str) -> Result<()> {
    let _ = reqwest::Client::new()
        .put("https://api.spotify.com/v1/me/player/pause")
        .bearer_auth(access_token)
        .header("Content-Length", "0")
        .send().await?;
    Ok(())
}

/// Resumes playback on the active device. Best-effort (see `pause`).
pub async fn resume(access_token: &str) -> Result<()> {
    let _ = reqwest::Client::new()
        .put("https://api.spotify.com/v1/me/player/play")
        .bearer_auth(access_token)
        .header("Content-Length", "0")
        .send().await?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_unix: i64,
}

#[derive(Deserialize)]
struct RawToken {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
}

pub fn generate_verifier() -> String {
    let mut rng = rand::thread_rng();
    (0..64).map(|_| UNRESERVED[rng.gen_range(0..UNRESERVED.len())] as char).collect()
}

pub fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn enc(s: &str) -> String {
    // Minimal application/x-www-form-urlencoded for URL query values.
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn authorize_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str, scopes: &str) -> String {
    format!(
        "https://accounts.spotify.com/authorize?client_id={}&response_type=code&redirect_uri={}&code_challenge_method=S256&code_challenge={}&state={}&scope={}",
        enc(client_id), enc(redirect_uri), enc(challenge), enc(state), enc(scopes)
    )
}

pub fn parse_callback_query(request_line: &str) -> Option<(String, String)> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", v)) => code = Some(v.to_string()),
            Some(("state", v)) => state = Some(v.to_string()),
            _ => {}
        }
    }
    Some((code?, state?))
}

pub fn parse_token_response(json: &str, fallback_refresh: Option<&str>, now_unix: i64) -> Result<Tokens> {
    let raw: RawToken = serde_json::from_str(json)?;
    let refresh_token = raw.refresh_token
        .or_else(|| fallback_refresh.map(|s| s.to_string()))
        .ok_or_else(|| anyhow!("no refresh token in response or fallback"))?;
    Ok(Tokens { access_token: raw.access_token, refresh_token, expires_at_unix: now_unix + raw.expires_in })
}

#[derive(Clone, Debug)]
pub struct NowPlaying {
    pub is_track: bool,
    pub spotify_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_art_url: Option<String>,
    pub duration_ms: i64,
    pub progress_ms: i64,
    pub is_playing: bool,
}

pub fn parse_now_playing(json: &str) -> Result<Option<NowPlaying>> {
    if json.trim().is_empty() { return Ok(None); }
    let v: serde_json::Value = serde_json::from_str(json)?;
    let cpt = v.get("currently_playing_type").and_then(|x| x.as_str()).unwrap_or("");
    let is_track = cpt == "track";
    let item = v.get("item");
    let get_str = |p: &str| item.and_then(|i| i.get(p)).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let album_art_url = item
        .and_then(|i| i.get("album")).and_then(|a| a.get("images")).and_then(|im| im.as_array())
        .and_then(|arr| arr.first()).and_then(|img| img.get("url")).and_then(|u| u.as_str())
        .map(|s| s.to_string());
    Ok(Some(NowPlaying {
        is_track,
        spotify_id: get_str("id"),
        title: get_str("name"),
        artist: item.and_then(|i| i.get("artists")).and_then(|a| a.as_array())
            .and_then(|arr| arr.first()).and_then(|a0| a0.get("name")).and_then(|n| n.as_str())
            .unwrap_or("").to_string(),
        album: item.and_then(|i| i.get("album")).and_then(|a| a.get("name")).and_then(|n| n.as_str())
            .unwrap_or("").to_string(),
        album_art_url,
        duration_ms: item.and_then(|i| i.get("duration_ms")).and_then(|d| d.as_i64()).unwrap_or(0),
        progress_ms: v.get("progress_ms").and_then(|p| p.as_i64()).unwrap_or(0),
        is_playing: v.get("is_playing").and_then(|b| b.as_bool()).unwrap_or(false),
    }))
}

fn now_unix() -> i64 { chrono::Utc::now().timestamp() }

pub async fn exchange_code(client_id: &str, code: &str, verifier: &str, redirect_uri: &str) -> Result<Tokens> {
    let body = [
        ("grant_type", "authorization_code"), ("code", code),
        ("redirect_uri", redirect_uri), ("client_id", client_id), ("code_verifier", verifier),
    ];
    let text = reqwest::Client::new()
        .post("https://accounts.spotify.com/api/token").form(&body).send().await?
        .text().await?;
    parse_token_response(&text, None, now_unix())
}

pub async fn refresh(client_id: &str, refresh_token: &str) -> Result<Tokens> {
    let body = [
        ("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id),
    ];
    let text = reqwest::Client::new()
        .post("https://accounts.spotify.com/api/token").form(&body).send().await?
        .text().await?;
    parse_token_response(&text, Some(refresh_token), now_unix())
}

pub async fn fetch_now_playing(access_token: &str) -> Result<Option<NowPlaying>> {
    let resp = reqwest::Client::new()
        .get("https://api.spotify.com/v1/me/player")
        .bearer_auth(access_token).send().await?;
    if resp.status().as_u16() == 204 { return Ok(None); }
    let text = resp.text().await?;
    parse_now_playing(&text)
}

pub fn load_tokens(store: &crate::store::Store) -> Option<Tokens> {
    let g = |k: &str| crate::store::get_setting(store, k).ok().flatten();
    Some(Tokens {
        access_token: g("access_token")?, refresh_token: g("refresh_token")?,
        expires_at_unix: g("expires_at")?.parse().ok()?,
    })
}
pub fn save_tokens(store: &crate::store::Store, t: &Tokens) {
    let _ = crate::store::set_setting(store, "access_token", &t.access_token);
    let _ = crate::store::set_setting(store, "refresh_token", &t.refresh_token);
    let _ = crate::store::set_setting(store, "expires_at", &t.expires_at_unix.to_string());
}

/// Blocks until Spotify redirects to the loopback (or `timeout` elapses), returns
/// (code, state). Uses a non-blocking accept loop so an abandoned login gives up the
/// port instead of holding it forever.
pub fn await_callback(port: u16, timeout: std::time::Duration) -> Result<(String, String)> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| anyhow!(
        "couldn't open the login listener on 127.0.0.1:{port} ({e}). Another login may still be \
         running — wait a few seconds and try Connect again."
    ))?;
    listener.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).ok();
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf)?;
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if let Some((code, state)) = parse_callback_query(first_line) {
                    let body = "<html><body style='background:#0D0D0F;color:#F2F2F3;font-family:sans-serif'>\
                        <h2>Samplify connected. You can close this tab.</h2></body></html>";
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
                    return Ok((code, state));
                } else {
                    let _ = write!(stream, "HTTP/1.1 204 No Content\r\n\r\n");
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(anyhow!("timed out waiting for Spotify to redirect back — please click Connect and try again"));
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_rfc7636_example() {
        // RFC 7636 Appendix B canonical vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(challenge_for(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn verifier_is_valid_length_and_charset() {
        let v = generate_verifier();
        assert!(v.len() >= 43 && v.len() <= 128);
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)));
    }

    #[test]
    fn authorize_url_has_pkce_params() {
        let url = authorize_url("CID", "http://127.0.0.1:5723/callback", "CHAL", "STATE",
            "user-read-playback-state user-read-currently-playing");
        assert!(url.starts_with("https://accounts.spotify.com/authorize?"));
        assert!(url.contains("client_id=CID"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5723%2Fcallback"));
        assert!(url.contains("state=STATE"));
    }

    #[test]
    fn parse_callback_extracts_code_and_state() {
        let line = "GET /callback?code=ABC123&state=xyz HTTP/1.1";
        assert_eq!(parse_callback_query(line), Some(("ABC123".into(), "xyz".into())));
        assert_eq!(parse_callback_query("GET /favicon.ico HTTP/1.1"), None);
    }

    #[test]
    fn await_callback_times_out_and_releases_port() {
        // No browser will connect, so it must give up near the deadline (not hang) and
        // free the port — the fix for an abandoned login wedging port 5723.
        let start = std::time::Instant::now();
        let r = await_callback(5799, std::time::Duration::from_millis(200));
        assert!(r.is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        // Port is free again immediately after.
        assert!(std::net::TcpListener::bind(("127.0.0.1", 5799)).is_ok());
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn token_response_computes_expiry() {
        let json = r#"{"access_token":"AAA","token_type":"Bearer","expires_in":3600,"refresh_token":"RRR","scope":"..."}"#;
        let t = parse_token_response(json, None, 1_000).unwrap();
        assert_eq!(t.access_token, "AAA");
        assert_eq!(t.refresh_token, "RRR");
        assert_eq!(t.expires_at_unix, 1_000 + 3600);
    }

    #[test]
    fn refresh_response_without_new_refresh_token_keeps_old() {
        let json = r#"{"access_token":"BBB","token_type":"Bearer","expires_in":3600,"scope":"..."}"#;
        let t = parse_token_response(json, Some("OLDREFRESH"), 0).unwrap();
        assert_eq!(t.refresh_token, "OLDREFRESH");
    }

    #[test]
    fn now_playing_track_parsed() {
        let json = r#"{"is_playing":true,"progress_ms":83000,"currently_playing_type":"track",
          "item":{"type":"track","id":"sp123","name":"Halo","duration_ms":270000,
            "artists":[{"name":"Catching Flies"}],
            "album":{"name":"Tides","images":[{"url":"http://img/large","height":640},{"url":"http://img/small","height":64}]}}}"#;
        let np = parse_now_playing(json).unwrap().unwrap();
        assert!(np.is_track);
        assert_eq!(np.spotify_id, "sp123");
        assert_eq!(np.title, "Halo");
        assert_eq!(np.artist, "Catching Flies");
        assert_eq!(np.progress_ms, 83000);
        assert_eq!(np.duration_ms, 270000);
        assert_eq!(np.album_art_url.as_deref(), Some("http://img/large"));
    }

    #[test]
    fn now_playing_ad_is_not_track() {
        let json = r#"{"is_playing":true,"progress_ms":5000,"currently_playing_type":"ad","item":null}"#;
        let np = parse_now_playing(json).unwrap().unwrap();
        assert!(!np.is_track);
    }

    #[test]
    fn empty_body_is_none() {
        assert!(parse_now_playing("").unwrap().is_none());
    }
}
