# Samplify — Spicetify extension

The extension adds Capture to the desktop playbar, puts saved notes on the seekbar and sends the current track and playback position to Samplify every half second. Samplify receives these updates at `ws://127.0.0.1:5724`. It accepts the desktop player’s browser origin `https://xpui.app.spotify.com` and local native clients without a browser Origin header. Leave both apps running while capturing.

## Install on Windows

1. Install the standalone Spotify desktop app from spotify.com.
2. In Samplify, open **Settings → Set up or repair Spotify**. Spotify restarts. The same setup is available from this directory as `setup.ps1`.
3. Play a track and click the orange **Capture** button in its playbar. Drag the two waveform edges, name the selection and save. The Library lists it under the track and artist.

If a Spotify update removes the button, run setup again. A fresh stock package needs a fresh Spicetify backup. The Windows helper and the app both use a checksum-pinned CLI in Samplify’s local tools folder.

For a manual installation, copy `samplify.js` into the Spicetify Extensions directory and run `spicetify config extensions samplify.js` followed by `spicetify apply` on an already patched client. The macOS bring-up still needs a live capture test.

## What it does

- **Position push** → the app's clock prefers this over API polling while the position is fresh.
- **Seekbar markers** — amber diamonds at each note's position; hover for the note
  text/tags, click to seek there.
- **Capture button** — records the Spotify process tree on Windows. The app button and hotkey use the source selected in Samplify Settings.

## Protocol (for reference)

Extension → app:
- `{"type":"position","spotify_id":"<id>","position_ms":<n>,"is_playing":<bool>}`
- `{"type":"request_marks","spotify_id":"<id>"}`
- `{"type":"capture"}`

App → extension:

- `{"type":"request_position"}` asks the player for a fresh position while it is in the background.
- `{"type":"capture_result","error":null}` confirms a capture. A string error explains why the capture failed.
- `{"type":"marks","spotify_id":"<id>","marks":[{"id","track_ms","note_text","tags"}]}`

## Troubleshooting

Spotify changes its player markup between versions. Check the playbar host when the button disappears and the progress-bar selectors when markers disappear. In Settings, “Receiving track details” confirms recent player messages.
