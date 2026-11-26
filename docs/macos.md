# macOS port

The initial target is **Apple Silicon, macOS 14 or newer**. ScreenCaptureKit capture, permission prompts and platform-specific file and URL handling have implementations. Windows tests exercise the shared code. Native macOS compilation and playback still require a Mac. Libraries are local to each machine.

## Build on a Mac

Install Xcode Command Line Tools, Rust through rustup and Node.js 24. From the repository root, run the following commands.

```sh
npm ci
npm run fetch-model
bash src-tauri/icons/build_mac_icon.sh
cargo test --manifest-path src-tauri/Cargo.toml
npm test
npm run tauri dev
```

Use `npm run tauri build -- --bundles dmg` to exercise packaging. The macOS release workflow is currently best-effort.

## Acceptance on Apple Silicon

| Area | Check |
| --- | --- |
| System audio | Grant, deny and regrant capture permission. Record distinct left and right reference signals. Check the WAV channels and duration after sleep, wake and an output-device change. Confirm the app reports a stopped capture stream. |
| Spotify | Install the extension in the standalone Mac player using Spicetify. Click Capture during playback, trim both ends and save. Verify the artist, track position, seekbar marker and reconnect after restarting both apps. Exercise repair after a Spotify update. |
| Playground | Combine a Spotify take with an imported WAV and AAC file. Change levels, stereo pan, timing and bar gates. Reopen the app and compare the four-bar bounce with the arrangement. |
| Files and hardware | Reveal a clip in Finder and drag the exported WAV into a DAW. Check that ONNX Runtime runs natively on arm64 and that the installed app finds its bundled model and separates a short stereo recording. |
| Distribution | Install from a clean DMG and upgrade with a populated library. Verify the bundle identity, file access and permission persistence. Sign and notarize the build before offering a supported Mac download. |

Record the Mac model, OS version, Spotify version and tested Git revision with the results. Intel support would require its own native runtime, package and playback tests. [Apple ScreenCaptureKit sample](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos) · [Tauri macOS application bundle](https://v2.tauri.app/distribute/macos-application-bundle/)
