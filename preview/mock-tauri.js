// Preview-only mock of the Tauri v2 global API, so the REAL library.js / popup.js
// run unmodified in a plain browser (via the Preview tool). Seeded with fake data.
// This file is NEVER shipped in the app — it lives only under preview/.
(function () {
  function writeStr(dv, off, s) { for (let i = 0; i < s.length; i++) dv.setUint8(off + i, s.charCodeAt(i)); }
  function silentWavUrl() {
    const sr = 8000, n = Math.floor(sr * 0.3);
    const buf = new ArrayBuffer(44 + n * 2), dv = new DataView(buf);
    writeStr(dv, 0, "RIFF"); dv.setUint32(4, 36 + n * 2, true); writeStr(dv, 8, "WAVE");
    writeStr(dv, 12, "fmt "); dv.setUint32(16, 16, true); dv.setUint16(20, 1, true); dv.setUint16(22, 1, true);
    dv.setUint32(24, sr, true); dv.setUint32(28, sr * 2, true); dv.setUint16(32, 2, true); dv.setUint16(34, 16, true);
    writeStr(dv, 36, "data"); dv.setUint32(40, n * 2, true);
    return URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
  }
  const SILENT = silentWavUrl();

  // Click-track WAV (60Hz bursts on the beat) so Playground BPM detection has
  // something real to find in the preview.
  function clickWavUrl(bpm, seconds) {
    const sr = 8000, n = Math.floor(sr * seconds);
    const buf = new ArrayBuffer(44 + n * 2), dv = new DataView(buf);
    writeStr(dv, 0, "RIFF"); dv.setUint32(4, 36 + n * 2, true); writeStr(dv, 8, "WAVE");
    writeStr(dv, 12, "fmt "); dv.setUint32(16, 16, true); dv.setUint16(20, 1, true); dv.setUint16(22, 1, true);
    dv.setUint32(24, sr, true); dv.setUint32(28, sr * 2, true); dv.setUint16(32, 2, true); dv.setUint16(34, 16, true);
    writeStr(dv, 36, "data"); dv.setUint32(40, n * 2, true);
    const period = 60 / bpm;
    for (let t = 0; t < seconds; t += period) {
      const start = Math.floor(t * sr);
      for (let i = 0; i < Math.floor(0.05 * sr) && start + i < n; i++) {
        const v = Math.sin((2 * Math.PI * 60 * i) / sr) * Math.exp(-i / (0.01 * sr));
        dv.setInt16(44 + (start + i) * 2, Math.round(v * 20000), true);
      }
    }
    return URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
  }
  const CLICK120 = clickWavUrl(120, 8);
  const CLICK90 = clickWavUrl(90, 8);

  // Steady tones so the stem mixer has four distinguishable lanes to play.
  function toneWavUrl(freq, seconds, amp) {
    const sr = 8000, n = Math.floor(sr * seconds);
    const buf = new ArrayBuffer(44 + n * 2), dv = new DataView(buf);
    writeStr(dv, 0, "RIFF"); dv.setUint32(4, 36 + n * 2, true); writeStr(dv, 8, "WAVE");
    writeStr(dv, 12, "fmt "); dv.setUint32(16, 16, true); dv.setUint16(20, 1, true); dv.setUint16(22, 1, true);
    dv.setUint32(24, sr, true); dv.setUint32(28, sr * 2, true); dv.setUint16(32, 2, true); dv.setUint16(34, 16, true);
    writeStr(dv, 36, "data"); dv.setUint32(40, n * 2, true);
    for (let i = 0; i < n; i++) {
      const env = 0.5 + 0.5 * Math.sin((i / sr) * 1.3); // slow swell so the waveform has shape
      dv.setInt16(44 + i * 2, Math.round(Math.sin((2 * Math.PI * freq * i) / sr) * amp * env * 20000), true);
    }
    return URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
  }
  const STEM_WAV = { drums: CLICK120, bass: toneWavUrl(55, 8, 0.9), other: toneWavUrl(330, 8, 0.5), vocals: toneWavUrl(660, 8, 0.7) };

  // Event bus so mock commands can push progress events like the backend does.
  const listeners = {};
  function emitMock(name, payload) {
    for (const cb of listeners[name] || []) { try { cb({ event: name, payload }); } catch (e) { console.error(e); } }
  }
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  let cancelSplit = false;
  let nextId = 5000;

  // Deterministic pseudo-waveform (no Math.random so it's stable across renders).
  function fakePeaks(n, seed) {
    const out = [];
    for (let i = 0; i < n; i++) {
      const t = i / n;
      const env = 0.35 + 0.65 * Math.pow(Math.sin(Math.PI * t), 0.6);
      const detail = 0.5 + 0.5 * Math.abs(Math.sin(i * (0.3 + seed * 0.017)) * Math.cos(i * 0.11));
      out.push(Math.min(1, env * detail));
    }
    return out;
  }

  // ---- Seed data ----
  const tags = ["break", "vocal", "melody", "bass", "chords", "texture", "fx"];
  let _collections = [{ id: 1, name: "Hip-Hop" }];
  const _colMembers = { 1: [101] };
  const tracks = [
    { track_id: 1, title: "STORM II", artist: "GENER8ION", album_art_url: null, mark_count: 5, latest_mark_at: "2026-07-05T02:10:00Z", favorite: true },
    { track_id: 2, title: "Superstar (UFC Freedom 250 Sean O'Malley Walkout)", artist: "UFC", album_art_url: null, mark_count: 3, latest_mark_at: "2026-07-05T01:40:00Z", favorite: false },
    { track_id: 3, title: "Vocal Flip", artist: "Logic", album_art_url: null, mark_count: 1, latest_mark_at: "2026-07-04T23:12:00Z", favorite: false },
    { track_id: 4, title: "Tides", artist: "Catching Flies", album_art_url: null, mark_count: 2, latest_mark_at: "2026-07-04T22:05:00Z", favorite: true },
  ];
  const detailByTrack = {
    1: {
      track: { id: 1, spotify_id: "sp1", title: "STORM II", artist: "GENER8ION", album: "STORM", album_art_url: null, duration_ms: 173000, favorite: true },
      sessions: [
        { id: 10, file_path: "C:/Users/User/Documents/Samplify/Clips/GENER8ION - STORM II [seed].wav", file_start_track_ms: 8000, sample_rate: 48000, channels: 2, duration_ms: 45000 },
        // Four stems already split from note 101 — exercise the mixer without running the model.
        { id: 11, file_path: "C:/clips/stems/drums.wav", file_start_track_ms: 10000, sample_rate: 48000, channels: 2, duration_ms: 8000 },
        { id: 12, file_path: "C:/clips/stems/bass.wav", file_start_track_ms: 10000, sample_rate: 48000, channels: 2, duration_ms: 8000 },
        { id: 13, file_path: "C:/clips/stems/other.wav", file_start_track_ms: 10000, sample_rate: 48000, channels: 2, duration_ms: 8000 },
        { id: 14, file_path: "C:/clips/stems/vocals.wav", file_start_track_ms: 10000, sample_rate: 48000, channels: 2, duration_ms: 8000 },
      ],
      marks: [
        { id: 101, track_ms: 10000, session_id: 10, session_offset_ms: 2000, note_text: "flip this break, pitch it down", tags: ["break", "melody"], rating: 4, key: "E minor", bpm: 82, created_at: "2026-07-05T02:00:00Z" },
        { id: 111, track_ms: 10000, session_id: 11, session_offset_ms: 0, note_text: "flip this break, pitch it down — drums", tags: ["break"], rating: null, key: null, bpm: null, stem: "drums", source_mark_id: 101, created_at: "2026-07-05T02:20:00Z" },
        { id: 112, track_ms: 10000, session_id: 12, session_offset_ms: 0, note_text: "flip this break, pitch it down — bass", tags: ["bass"], rating: null, key: null, bpm: null, stem: "bass", source_mark_id: 101, created_at: "2026-07-05T02:20:00Z" },
        { id: 113, track_ms: 10000, session_id: 13, session_offset_ms: 0, note_text: "flip this break, pitch it down — other", tags: ["texture"], rating: null, key: null, bpm: null, stem: "other", source_mark_id: 101, created_at: "2026-07-05T02:20:00Z" },
        { id: 114, track_ms: 10000, session_id: 14, session_offset_ms: 0, note_text: "flip this break, pitch it down — vocals", tags: ["vocal"], rating: null, key: null, bpm: null, stem: "vocals", source_mark_id: 101, created_at: "2026-07-05T02:20:00Z" },
        { id: 102, track_ms: 22000, session_id: 10, session_offset_ms: 14000, note_text: "vocal chop potential", tags: ["vocal"], rating: 3, key: "G", bpm: 110, created_at: "2026-07-05T02:02:00Z" },
        { id: 103, track_ms: 31000, session_id: 10, session_offset_ms: 23000, note_text: "slice and reverse this fill", tags: ["break", "fx"], rating: null, key: null, bpm: null, created_at: "2026-07-05T02:05:00Z" },
        { id: 104, track_ms: 40000, session_id: 10, session_offset_ms: 32000, note_text: "nice chord stab", tags: ["chords"], rating: 5, key: null, bpm: null, created_at: "2026-07-05T02:08:00Z" },
        { id: 105, track_ms: 50000, session_id: 10, session_offset_ms: 42000, note_text: "outro texture idea", tags: ["texture", "fx"], rating: null, key: null, bpm: null, created_at: "2026-07-05T02:10:00Z" },
      ],
    },
    // Two separate recordings of the SAME song (two capture sessions), each its own
    // file — reproduces the multi-session playback bug: every note must play its OWN clip.
    2: {
      track: { id: 2, spotify_id: "sp2", title: "Superstar", artist: "UFC", album: "", album_art_url: null, duration_ms: 200000, favorite: false },
      sessions: [
        { id: 20, file_path: "C:/clips/superstar_A.wav", file_start_track_ms: 30000, sample_rate: 48000, channels: 2, duration_ms: 8000 },
        { id: 21, file_path: "C:/clips/superstar_B.wav", file_start_track_ms: 90000, sample_rate: 48000, channels: 2, duration_ms: 7000 },
      ],
      marks: [
        { id: 201, track_ms: 32000, session_id: 20, session_offset_ms: 2000, trim_start_ms: 1000, trim_end_ms: 5000, note_text: "clip A note", tags: ["break"], rating: null, key: null, bpm: null, created_at: "2026-07-07T01:00:00Z" },
        { id: 202, track_ms: 92000, session_id: 21, session_offset_ms: 2000, trim_start_ms: 1000, trim_end_ms: 6000, note_text: "clip B note", tags: ["vocal"], rating: null, key: null, bpm: null, created_at: "2026-07-07T01:05:00Z" },
      ],
    },
  };
  function makeDetail(id) {
    if (detailByTrack[id]) return detailByTrack[id];
    const t = tracks.find((x) => x.track_id === id) || tracks[0];
    return {
      track: { id, spotify_id: "sp" + id, title: t.title, artist: t.artist, album: "", album_art_url: null, duration_ms: 200000, favorite: t.favorite },
      sessions: [{ id: 100 + id, file_path: "C:/clip" + id + ".wav", file_start_track_ms: 0, sample_rate: 48000, channels: 2, duration_ms: 30000 }],
      marks: [{ id: 900 + id, track_ms: 12000, session_id: 100 + id, session_offset_ms: 12000, note_text: "sample idea", tags: ["vocal"], rating: null, key: null, bpm: null, created_at: "2026-07-04T23:00:00Z" }],
    };
  }

  function findMark(id) {
    for (const key of Object.keys(detailByTrack)) {
      const d = detailByTrack[key];
      const m = (d.marks || []).find((x) => x.id === id);
      if (m) return { d, m };
    }
    return null;
  }
  const RAW = ["drums", "bass", "other", "vocals"];

  const handlers = {
    get_storage_info: () => ({ free_gb: 46.9, total_gb: 925.2 }),
    get_settings: () => ({ client_id: "", buffer_seconds: 120, authenticated: true, capture_hotkey: "Ctrl+Shift+P", debug: true, version: "1.0.0-preview" }),
    check_capture_status: () => ({ authorized: true, platform: "windows", denied_reason: null }),
    spotify_install_kind: () => "standalone",
    // --- stems mock ---
    stem_model_status: () => ({ present: true, path: "C:/Program Files/Samplify/resources/models/htdemucs_fp16weights.onnx", bytes: 165612636, max_clip_seconds: 60 }),
    get_stem_set: (a) => {
      const hit = findMark(a.markId);
      if (!hit) throw new Error("unknown mark " + a.markId);
      const sourceId = RAW.includes(hit.m.stem) ? hit.m.source_mark_id : hit.m.id;
      const src = findMark(sourceId);
      if (!src) return null;
      const stems = RAW.map((name) => {
        const m = [...src.d.marks].reverse().find((x) => x.source_mark_id === sourceId && x.stem === name);
        if (!m) return null;
        const se = src.d.sessions.find((s) => s.id === m.session_id);
        return { mark_id: m.id, session_id: m.session_id, stem: name, file_path: se ? se.file_path : "", sample_rate: 48000, duration_ms: se ? se.duration_ms : null };
      }).filter(Boolean);
      if (!stems.length) return null;
      const se = src.d.sessions.find((s) => s.id === src.m.session_id);
      return {
        source_mark_id: sourceId, track_id: src.d.track.id, title: src.d.track.title, artist: src.d.track.artist,
        note_text: src.m.note_text, track_ms: (se ? se.file_start_track_ms : 0) + (src.m.trim_start_ms || 0),
        bpm: src.m.bpm || null, key: src.m.key || null, stems,
      };
    },
    separate_mark_stems: async (a) => {
      const hit = findMark(a.markId);
      if (!hit) throw new Error("unknown mark");
      cancelSplit = false;
      const total = 6;
      for (let i = 1; i <= total; i++) {
        await sleep(220);
        if (cancelSplit) throw new Error("separation cancelled");
        emitMock("stem-progress", { markId: a.markId, done: i, total });
      }
      const se = hit.d.sessions.find((s) => s.id === hit.m.session_id);
      const ids = [];
      for (const name of RAW) {
        const sid = nextId++;
        hit.d.sessions.push({ id: sid, file_path: `C:/clips/stems/${name}.wav`, file_start_track_ms: se ? se.file_start_track_ms : 0, sample_rate: 48000, channels: 2, duration_ms: 8000 });
        const mid = nextId++;
        hit.d.marks.push({ id: mid, track_ms: hit.m.track_ms, session_id: sid, session_offset_ms: 0, note_text: `${hit.m.note_text} — ${name}`, tags: [], rating: null, key: null, bpm: null, stem: name, source_mark_id: hit.m.id, created_at: "2026-09-05T00:00:00Z" });
        ids.push(mid);
      }
      return ids;
    },
    cancel_stem_separation: () => { cancelSplit = true; return null; },
    export_stem_mix: (a) => {
      const hit = findMark(a.sourceMarkId);
      const inc = Object.keys(a.gains || {}).filter((k) => a.gains[k] > 0);
      const label = inc.length === 4 ? "mix" : `mix ${inc.join("+")}`;
      if (a.saveAsNote) {
        const sid = nextId++, mid = nextId++;
        hit.d.sessions.push({ id: sid, file_path: `C:/clips/${label}.wav`, file_start_track_ms: hit.m.track_ms, sample_rate: 48000, channels: 2, duration_ms: 8000 });
        hit.d.marks.push({ id: mid, track_ms: hit.m.track_ms, session_id: sid, session_offset_ms: 0, note_text: `${hit.m.note_text} — ${inc.length === 4 ? "mix" : "mix (" + inc.join(" + ") + ")"}`, tags: [], rating: null, key: null, bpm: null, stem: "mix", source_mark_id: hit.m.id, created_at: "2026-09-05T00:00:00Z" });
        return { path: `C:/clips/${label}.wav`, mark_id: mid };
      }
      return { path: `C:/Users/User/Documents/Samplify/Exports/GENER8ION - STORM II [0.10 ${label}].wav` };
    },
    prepare_stem_mix_drag: (a) => `C:/temp/samplify-drag/mix-${a.sourceMarkId}.wav`,
    delete_stem_set: (a) => {
      const hit = findMark(a.sourceMarkId);
      const gone = hit.d.marks.filter((m) => m.source_mark_id === a.sourceMarkId && RAW.includes(m.stem));
      hit.d.marks = hit.d.marks.filter((m) => !gone.includes(m));
      hit.d.sessions = hit.d.sessions.filter((s) => !gone.some((m) => m.session_id === s.id));
      return null;
    },
    save_playground_take: (bytes, options) => {
      const headers = options.headers;
      const title = new TextDecoder().decode(Uint8Array.from(atob(headers["x-samplify-title"]), c => c.charCodeAt(0)));
      const artist = headers["x-samplify-kind"] === "mix" ? "Playground mix" : "Imported audio";
      const id = nextId++, sid = nextId++, mid = nextId++;
      const created = new Date().toISOString();
      const view = new DataView(bytes);
      const rate = view.getUint32(24, true), channels = view.getUint16(22, true);
      const duration = Math.round(view.getUint32(40, true) * 1000 / (rate * channels * 2));
      const url = URL.createObjectURL(new Blob([bytes], {type: "audio/wav"}));
      tracks.push({track_id: id, title, artist, mark_count: 1, latest_mark_at: created});
      detailByTrack[id] = {
        track: {id, title, artist, duration_ms: duration},
        sessions: [{id: sid, file_path: url, sample_rate: rate, channels, duration_ms: duration, file_start_track_ms: 0}],
        marks: [{id: mid, track_id: id, session_id: sid, track_ms: 0, session_offset_ms: 0, note_text: title, tags: [], created_at: created}],
      };
      emitMock("mark-created", {mark_id: mid, want_popup: false});
      return mid;
    },
    get_library: () => tracks.map((t) => ({ ...t })),
    get_tracks_by_tag: (a) => tracks.filter((t) => {
      const d = detailByTrack[t.track_id];
      return d && (d.marks || []).some((m) => (m.tags || []).includes(a.tag));
    }).map((t) => ({ ...t })),
    // --- collections mock ---
    list_collections: () => _collections.map((c) => ({ id: c.id, name: c.name, clip_count: (_colMembers[c.id] || []).length })),
    create_collection: (a) => { const id = _collections.length ? Math.max(..._collections.map((c) => c.id)) + 1 : 1; _collections.push({ id, name: a.name }); _colMembers[id] = []; return id; },
    delete_collection: (a) => { _collections = _collections.filter((c) => c.id !== a.id); delete _colMembers[a.id]; return null; },
    rename_collection: () => null,
    set_mark_in_collection: (a) => { _colMembers[a.collectionId] = _colMembers[a.collectionId] || []; if (a.member) { if (!_colMembers[a.collectionId].includes(a.markId)) _colMembers[a.collectionId].push(a.markId); } else { _colMembers[a.collectionId] = _colMembers[a.collectionId].filter((m) => m !== a.markId); } return null; },
    collections_for_mark: (a) => _collections.filter((c) => (_colMembers[c.id] || []).includes(a.markId)).map((c) => c.id),
    get_collection_clips: (a) => (_colMembers[a.collectionId] || []).map((mid, i) => ({ mark_id: mid, title: "STORM II", artist: "GENER8ION", note_text: "Vocals", track_ms: 8000, duration_ms: 45000, rating: 4, tags: ["vocal"], file_path: "C:/fake.wav", trim_start_ms: null, trim_end_ms: null, album_art_url: null })),
    prepare_collection_export: (a) => (_colMembers[a.collectionId] || []).map((_, i) => `C:/Packs/clip_${i}.wav`),
    search_marks: (a) => { const q = (a.query || "").toLowerCase(); return detailByTrack[1].marks.filter((m) => m.note_text.toLowerCase().includes(q)).map((m) => ({ ...m, track_id: 1, title: "STORM II", artist: "GENER8ION" })); },
    get_track_detail: (a) => makeDetail(a.trackId),
    get_marks_by_ids: (a) => {
      const out = [];
      for (const id of a.ids || []) {
        for (const key of Object.keys(detailByTrack)) {
          const d = detailByTrack[key];
          const m = (d.marks || []).find((x) => x.id === id);
          if (m && m.session_id != null) {
            const se = (d.sessions || []).find((s) => s.id === m.session_id);
            out.push({
              mark_id: m.id, track_id: d.track.id, title: d.track.title, artist: d.track.artist,
              note_text: m.note_text, tags: m.tags || [], rating: m.rating ?? null, key: m.key ?? null,
              bpm: m.bpm ?? null, file_path: se ? se.file_path : "C:/missing.wav",
              trim_start_ms: m.trim_start_ms ?? null, trim_end_ms: m.trim_end_ms ?? null,
              duration_ms: se ? se.duration_ms : null,
            });
          }
        }
      }
      return out;
    },
    prepare_drag_wav: (a) => {
      for (const key of Object.keys(detailByTrack)) {
        const d = detailByTrack[key];
        const m = (d.marks || []).find((x) => x.id === a.markId);
        if (!m) continue;
        const se = (d.sessions || []).find((s) => s.id === m.session_id);
        if (!se) break;
        return m.trim_start_ms != null && m.trim_end_ms != null
          ? `C:/temp/samplify-drag/mark-${m.id}-${m.trim_start_ms}-${m.trim_end_ms}.wav`
          : se.file_path;
      }
      throw new Error("no audio for mark " + a.markId);
    },
    create_mark_from_selection: (a) => {
      // Mirrors the Rust command: find the session, insert a mark against it,
      // derive track_ms from the capture's start. No audio is touched.
      for (const key of Object.keys(detailByTrack)) {
        const d = detailByTrack[key];
        const se = (d.sessions || []).find((s) => s.id === a.sessionId);
        if (!se) continue;
        const id = Math.max(0, ...d.marks.map((m) => m.id)) + 1;
        d.marks.push({
          id,
          track_ms: (se.file_start_track_ms || 0) + a.trimStartMs,
          session_id: se.id,
          session_offset_ms: a.trimStartMs,
          trim_start_ms: a.trimStartMs,
          trim_end_ms: a.trimEndMs,
          note_text: a.noteText || "",
          tags: a.tags || [],
          rating: null, key: null, bpm: null,
          created_at: "2026-07-25T12:00:00Z",
        });
        return id;
      }
      throw new Error("unknown capture session " + a.sessionId);
    },
    get_waveform: (a) => fakePeaks(a.buckets || 600, a.sessionId || 1),
    get_tags: () => tags.slice(),
    update_mark_meta: () => null,
    delete_mark: () => null,
    set_favorite: () => null,
    delete_track: () => null,
    export_mark: () => "C:/Users/User/Documents/Samplify/Exports/GENER8ION - STORM II [0.10 (break,melody)].wav",
    set_hotkeys: () => null,
    "plugin:global-shortcut|is_registered": () => true,
    save_client_id: () => null,
    save_settings: () => null,
    begin_auth: () => null,
    seed_dev_data: () => null,
    trigger_capture_cmd: () => null,
    save_capture_cmd: () => null,
    discard_capture_cmd: () => null,
    popup_ready: () => ({ title: "STORM II", artist: "GENER8ION", trigger_track_ms: 52000, snapshot_duration_ms: 45000, temp_wav_path: "temp", waveform: fakePeaks(2000, 3), album_art_url: null }),
  };

  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args, options) => {
        const h = handlers[cmd];
        if (!h) { console.warn("[mock] unhandled invoke:", cmd, args); return null; }
        return h(args || {}, options);
      },
      convertFileSrc: (p) => {
        if (String(p).startsWith("blob:")) return p;
        const m = /stems\/(drums|bass|other|vocals)\.wav$/.exec(String(p));
        if (m) return STEM_WAV[m[1]];
        return /superstar_B/.test(String(p)) ? CLICK90 : CLICK120;
      },
      Channel: class { constructor() { this.onmessage = null; } },
    },
    event: {
      listen: async (name, cb) => {
        (listeners[name] = listeners[name] || []).push(cb);
        return () => { listeners[name] = (listeners[name] || []).filter((f) => f !== cb); };
      },
      emit: async (name, payload) => emitMock(name, payload),
    },
    window: {
      getCurrentWindow: () => ({
        hide: async () => { document.body.innerHTML = '<div style="color:#8A8A90;font-family:sans-serif;padding:40px">[capture card hidden — reload to reopen]</div>'; },
        show: async () => {},
        setFocus: async () => {},
        onFocusChanged: () => {},
      }),
    },
  };
})();
