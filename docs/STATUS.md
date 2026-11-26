# Verification status

## Windows x64 · 20 November 2026

| Path | Evidence |
| --- | --- |
| Windows 1.0.0 stale-buffer reproduction | With Spotify actively rendering, captures near 0:57 and 1:39 of the same song produced byte-identical 19.36-second WAVs. The first 15 one-second RMS measurements rounded to zero. The old endpoint stream retained its samples indefinitely after input stopped. |
| Windows 1.0.1 Spotify isolation | A 52.28-second capture of the same passage matched an independent speaker-loopback recording at four points. A second app played a known two-second sound during the passage. Subtracting the aligned Spotify recording from the speaker mix recovered the test sound with 0.99979 correlation. |
| Final 1.0.1 installer and playbar | Upgraded an existing library and compared the installed executable to the NSIS payload by SHA-256. After switching songs and minimizing Spotify for 50 seconds, invoking the actual playbar button opened a 54.00-second stereo capture. Four two-second passages matched the independent output at the same offset. The test notification was detected on the endpoint and excluded from the Spotify take. SQLite integrity, track/session/note counts and the source preference were checked after the test. |
| Explicit System audio | Selecting System audio in Settings, playing the same test sound from another process and pressing the app's Capture button produced a system-labeled WAV with the test sound at 0.9991 of its reference level. Spotify remained paused. Switching back restored Spotify-only capture. |
| Spotify → capture card | On the standalone Spotify 1.3.10.2586 player, Spicetify 2.46.7 restored the playbar button after a client update. Two clicks during playback opened the native card. Dragging both edges produced 4.5- and 5.9-second takes with the expected artist, track and position, without Web API credentials. |
| Capture and audition | On Windows 11, two successive takes produced different WAVs. Audition bytes matched each frozen WAV by SHA-256. A four-second stereo selection contained the expected 192,000 frames. |
| Library → stems → DAW | The packaged model produced four stems. The solo drum bounce matched the raw drum stem within one PCM count. A Windows file-drop receiver received a WAV identical to the export. Audacity reopened a Playground mix as a stereo track and played it. |
| Playground arrangement | A Spotify take and imported drums were arranged at 120 BPM. The export contained 384,000 stereo frames at 48 kHz. Per-bar channel measurements confirmed left only, right only, both and silence. Restarting restored the lane order, bar switches, pan, mute and mix name. |
| Final installer payload | The extracted executable ran with an empty profile and received fresh track updates from the running Spotify client after the bridge origin check. A collection name containing quotes, angle brackets and an ampersand displayed literally. Two imported score stems produced an 88 BPM bounce of 523,636 stereo frames at 48 kHz. The saved library WAV and export matched byte for byte. |
| Automated tests | All 91 Rust library tests passed with both real-model tests enabled. The suite covers frame alignment, capture timing, bridge origin checks, SQLite transactions, stem separation and WAV output. All 12 JavaScript tests passed for tempo detection, loop timing and PCM encoding. |

The Windows package and model-backed tests used Rust 1.94.1. Rust 1.100 changed the build directory layout used by the DLL-copy step in `ort-sys` 2.0.0-rc.12. A toolchain upgrade needs a clean Windows build as well as a native stem split.

The 1.0.1 Windows installer’s SHA-256 is `91477b5fb8c2e2998f6d1bc48ecea836abd24a2a40c855114bb147f135b9e871`. The earlier 1.0.0 Windows installer’s SHA-256 is `15e3ad6d46ed9f40b48d04df155bd2fe4b87883077e80088c7b954f42cf68376`. Inspection of the NSIS payload found the pinned model, the Samplify and Demucs licenses, and the ONNX Runtime license and notices.

## Remaining release checks

- Run the installer wizard on a clean Windows account or VM. The in-place Windows upgrade runs above used an existing library.
- Audition loop seams and stem bleed on a listening setup. Verify drag-in and timing in Ableton Live or FL Studio.
- Run the native build, ScreenCaptureKit permission flows, Spotify setup, export and installation on an Apple Silicon Mac. [macOS acceptance plan](macos.md)

[Feature film and soundtrack credits](demo/README.md) · [Architecture](architecture.md)
