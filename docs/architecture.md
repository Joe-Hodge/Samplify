# Architecture

Samplify runs a Tauri desktop process with two local webviews. The Library owns playback, the Stem Mixer and Playground. The capture card opens against a frozen snapshot, so changing the selection never changes the sound under its handles.

| Boundary | Implementation | Responsibility |
| --- | --- | --- |
| Audio input | [Capture engine](../src-tauri/src/capture.rs), [Windows](../src-tauri/src/capture_win.rs), [macOS](../src-tauri/src/capture_mac.rs) | Feed interleaved, frame-aligned samples into the rolling memory buffer. Windows uses WASAPI process loopback for the local Spotify process tree. The explicit System audio setting starts a separate default-output stream. Each worker reconnects when its source changes. The Mac port uses ScreenCaptureKit. |
| Spotify | [Extension](../spicetify-ext/samplify.js), [bridge](../src-tauri/src/bridge.rs), [clock](../src-tauri/src/clock.rs) | The extension sends the track, position and playback state to a loopback WebSocket. The clock interpolates between updates. An optional PKCE connection supplies Web API metadata. |
| Capture and library | [Commands](../src-tauri/src/commands.rs), [store](../src-tauri/src/store.rs) | Freeze the buffer, write a temporary audition WAV, map the selection to song time and save it. SQLite stores tracks, sessions, notes, tags and collections. FTS5 indexes note text. |
| Stem separation | [Pipeline](../src-tauri/src/stems/mod.rs), [job control](../src-tauri/src/stems/job.rs), [mixer](../src/js/stems.js) | Resample to 44.1 kHz, separate overlapping chunks through ONNX Runtime and return four WAVs aligned to the source. A single job owns the model at a time. Each saved stem or mix points back to its source note. |
| Arrangement | [Playground](../src/js/playground.js), [audio graph](../src/js/arrangement.js), [native saves](../src-tauri/src/playground.rs) | Schedule looping sources, gain, stereo pan and four bar gates against one audio clock. Playback and offline rendering share the graph. The backend validates PCM before saving an import or bounce to the library. |

Windows process capture follows the current Spotify process tree across output devices. A native heartbeat requests track updates while Spotify is in the background. The system source follows the default output device selected in Windows. [Microsoft application loopback sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/)

## Following a capture

1. The Windows playbar always requests Spotify audio. The app button and hotkey follow the selected source. The Spotify buffer tracks playback changes and packet timestamps, drops old samples at track changes, seeks, restarts and input gaps, and holds the last phrase on pause. A stale or silent source returns an error. System captures have their own local identity and timeline. The popup signals readiness before it receives its frozen snapshot.
2. Saving the selection writes its WAV and associates it with a session and note. Later trims record offsets. The source audio remains available.
3. Sending a note or stem to Playground references its audio. Importing a recording decodes it through the webview and persists a WAV. The local arrangement retains lane order, timing, bar choices and mix controls across restarts.
4. Saving a four-bar mix renders stereo PCM through an offline audio context. The saved mix is a library item. Export writes a copy for the user’s DAW.

## Local data and network

By default, audio lives in `Documents/Samplify/Clips` and exports in `Documents/Samplify/Exports`. The app data directory holds `samplify.db` in WAL mode. Optional Web API credentials are stored in the local database. Treat a library backup as personal data.

The extension connects to `127.0.0.1:5724`. The bridge accepts the Spotify desktop browser origin and native local clients. OAuth uses the separate loopback callback `http://127.0.0.1:5723/callback`. To use a personal Web API client, register that redirect URI and paste the public client ID under **Settings → Advanced**. [Spotify Developer Dashboard](https://developer.spotify.com/dashboard)
