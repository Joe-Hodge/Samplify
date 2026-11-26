import { invoke, listen } from "./bridge.js";
import { TAGS, fmtTime } from "./tags.js";
import { WaveformSelector } from "./waveform.js";
import { initPlayground, renderPlayground, playgroundAddClip, stopAll as stopPlayground } from "./playground.js";
import { icon, mountIcons } from "./icons.js";
import { initStemMixer, openStemMixer, closeStemMixer, isStemMixerOpen, stemMixerSourceId, stemMixerPause, mixerSoloedPlaygroundClips, RAW_STEMS } from "./stems.js";
import { initVisualizer, setSamplePeaks, setAudioActive, connectAudioSource, updateVisualizerPlayhead } from "./visualizer.js";

// DOM Cache
const tracksListEl = document.getElementById("track-list");
const sampleCountEl = document.getElementById("sample-count");
const searchEl = document.getElementById("search");
const detailViewEl = document.getElementById("detail-view");
const detailEmptyEl = document.getElementById("detail-empty");
const detailContentEl = document.getElementById("detail-content");

const detailTrackTitleEl = document.getElementById("detail-track-title");
const detailTrackMetaEl = document.getElementById("detail-track-meta");
const waveformCanvas = document.getElementById("waveform-canvas");
const waveformPinsEl = document.getElementById("waveform-pins");
const waveformPlaceholder = document.getElementById("waveform-placeholder");
const timelineStartEl = document.getElementById("timeline-start");
const timelineEndEl = document.getElementById("timeline-end");
const notesListEl = document.getElementById("notes-list");
const noteCountTitleEl = document.getElementById("note-count-title");
const sortSelectEl = document.getElementById("sort-select");
const favStarEl = document.getElementById("fav-star");
const trackDeleteEl = document.getElementById("track-delete");

const selBarEl = document.getElementById("sel-bar");
const selBarReadoutEl = document.getElementById("sel-bar-readout");
const selBarNoteEl = document.getElementById("sel-bar-note");
const selBarSaveNewEl = document.getElementById("sel-bar-save-new");
const selBarUpdateEl = document.getElementById("sel-bar-update");
const selBarRevertEl = document.getElementById("sel-bar-revert");
const captureStripEl = document.getElementById("capture-strip");

// Bottom Transport DOM
const tbArtEl = document.getElementById("tb-art");
const tbTitleEl = document.getElementById("tb-title");
const tbArtistEl = document.getElementById("tb-artist");
const tbPlayPauseEl = document.getElementById("tb-play-pause");
const tbCurrentTimeEl = document.getElementById("tb-current-time");
const tbDurationEl = document.getElementById("tb-duration");
const tbVolumeEl = document.getElementById("tb-volume");
function escapeText(value) {
  const span = document.createElement("span");
  span.textContent = value ?? "";
  return span.innerHTML.replaceAll('"', "&quot;").replaceAll("'", "&#39;");
}

const audioEl = document.getElementById("clip-audio");
function setTbPlaying(on) { tbPlayPauseEl.innerHTML = icon(on ? "pause" : "play", 16); }
audioEl.addEventListener("play", () => {
  // Pause the mixer first so its onPlaybackStateChange(false) cannot clobber
  // the live flag we set next.
  stopPlayground();
  stemMixerPause();
  setTbPlaying(true);
  setAudioActive(true);
});
audioEl.addEventListener("pause", () => {
  setTbPlaying(false);
  setAudioActive(false);
  updatePlayStates();
});
audioEl.addEventListener("error", () => {
  if (!audioEl.src || audioEl.src === window.location.href) return;
  const err = audioEl.error;
  if (!err) return;
  console.warn(`Audio error (code ${err.code}):`, err.message);
  const msg = {
    1: "Audio playback aborted",
    2: "Audio file could not be loaded",
    3: "Audio decode error: file could not be read",
    4: "Audio format not supported",
  }[err.code];
  if (msg) showToast(msg);
});


// Views
const views = {
  library: document.getElementById("detail-view"),
  settings: document.getElementById("settings-view"),
  tags: document.getElementById("tags-view"),
  collections: document.getElementById("collections-view"),
  playground: document.getElementById("playground-view"),
};

// State
let selectedTrackId = null;
let selectedMarkId = null;
let currentTrackDetail = null;
let activeTagFilter = null; // when set, the sample list shows only tracks with this tag
let currentSession = null;
let sortMode = "recent";
// Whether the bundled separation model resolved — split buttons are disabled
// with a reason when it did not, instead of failing on click.
let stemModel = { present: true, path: "", bytes: 0, max_clip_seconds: 60 };

// Sort the library track list by the active mode.
function sortTracks(tracks) {
  const arr = [...tracks];
  const recent = (a, b) => String(b.latest_mark_at).localeCompare(String(a.latest_mark_at));
  if (sortMode === "az") arr.sort((a, b) => a.title.localeCompare(b.title));
  else if (sortMode === "most") arr.sort((a, b) => b.mark_count - a.mark_count || recent(a, b));
  else if (sortMode === "fav") arr.sort((a, b) => (b.favorite ? 1 : 0) - (a.favorite ? 1 : 0) || recent(a, b));
  else arr.sort(recent);
  return arr;
}

// Toast helper
function showToast(msg) {
  const container = document.getElementById("toast-container");
  const el = document.createElement("div");
  el.className = "toast";
  el.textContent = msg;
  container.appendChild(el);
  setTimeout(() => {
    el.classList.add("fade-out");
    setTimeout(() => el.remove(), 200);
  }, 2000);
}

// Format duration helper (mm:ss)
function fmtSecs(secs) {
  if (isNaN(secs)) return "0:00";
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

function trimReadoutText(mark) {
  if (mark.trim_start_ms != null && mark.trim_end_ms != null) {
    return `In ${fmtSecs(mark.trim_start_ms / 1000)} · Out ${fmtSecs(mark.trim_end_ms / 1000)} · ${fmtSecs((mark.trim_end_ms - mark.trim_start_ms) / 1000)}`;
  }
  return "Full clip";
}

// Navigate
function navigateTo(viewName) {
  Object.keys(views).forEach(k => {
    views[k].style.display = k === viewName ? "block" : "none";
  });
  // Active classes on left rail
  document.querySelectorAll(".nav-item").forEach(el => {
    const target = el.getAttribute("data-view");
    if (target === viewName) el.classList.add("active");
    else el.classList.remove("active");
  });

  if (viewName === "settings") {
    loadSettings();
  } else if (viewName === "tags") {
    renderTagsView();
  } else if (viewName === "collections") {
    renderCollections();
  } else if (viewName === "playground") {
    renderPlayground();
  } else if (viewName === "library") {
    renderLibrary();
  }
}

document.querySelectorAll(".nav-item").forEach(item => {
  item.onclick = (e) => {
    e.preventDefault();
    const viewName = item.getAttribute("data-view");
    if (item.classList.contains("muted-item")) {
      showToast(`${viewName.toUpperCase()}: coming in Phase 3`);
      return;
    }
    navigateTo(viewName);
  };
});

document.getElementById("btn-settings").onclick = () => navigateTo("settings");
document.getElementById("btn-settings-back").onclick = () => navigateTo("library");
document.getElementById("btn-tags-back").onclick = () => navigateTo("library");

// Load storage info
async function loadStorageInfo() {
  try {
    const info = await invoke("get_storage_info");
    const el = document.getElementById("storage-text");
    el.textContent = `${info.free_gb.toFixed(1)} GB free`;
    el.title = `${info.free_gb.toFixed(1)} GB free of ${info.total_gb.toFixed(1)} GB on the clips drive`;
  } catch (e) {
    document.getElementById("storage-text").textContent = "Free space: -- GB";
  }
}

// Check system-audio capture authorization (macOS-only path; Windows always
// returns authorized=true so the banner/Settings row stay hidden). The Rust
// cmd `check_capture_status` returns {authorized, platform, denied_reason}.
// On macOS when authorization is anything but Authorized we show:
//   - the empty-state banner with Open System Settings button, and
//   - the Settings row reflecting live status.
// Both buttons call `open_screen_recording_settings` which deep-links
// System Settings -> Privacy & Security -> Screen Recording. macOS stops
// re-prompting the TCC sheet after a denial, so the user must toggle there
// manually then restart Samplify (the banner text says so).
let capturePlatform = "windows"; // set on first poll
let captureAuthorized = true;
async function checkCaptureStatus() {
  try {
    const s = await invoke("check_capture_status");
    capturePlatform = s.platform || "windows";
    captureAuthorized = !!s.authorized;
    const isMac = capturePlatform === "macos";

    // Settings row (macOS only; on Windows it stays hidden by its inline
    // style="display:none" and we never touch it).
    const macRow = document.getElementById("mac-capture-settings");
    if (macRow) macRow.style.display = isMac ? "block" : "none";
    if (isMac) {
      const statusEl = document.getElementById("mac-capture-status");
      if (statusEl) {
        statusEl.textContent = captureAuthorized
          ? "Authorized"
          : (s.denied_reason || "Not authorized \u2014 open System Settings to grant Screen Recording.");
      }
    }

    // Empty-state banner (shown when the Library view has no selection AND
    // capture is not authorized on macOS). On Windows we never show it.
    const banner = document.getElementById("capture-permission-banner");
    if (banner) {
      const show = isMac && !captureAuthorized;
      banner.style.display = show ? "block" : "none";
      const txt = document.getElementById("capture-permission-text");
      if (txt && s.denied_reason) txt.textContent = s.denied_reason;
    }
  } catch (e) {
    // Rust cmd unavailable (older backend) -> assume Windows-like (authorized
    // but platform unknown) so the UI never blocks capture unexpectedly.
    capturePlatform = "windows"; captureAuthorized = true;
  }
}

// The two "Open System Settings" buttons share a single handler.
function wireOpenScreenSettingsButtons() {
  for (const id of ["btn-open-screen-settings", "btn-open-screen-settings-2"]) {
    const el = document.getElementById(id);
    if (el && !el.dataset.wired) {
      el.dataset.wired = "1";
      el.onclick = async () => {
        try { await invoke("open_screen_recording_settings"); }
        catch (e) { showToast(`Could not open System Settings: ${e}`); }
      };
    }
  }
}
wireOpenScreenSettingsButtons();

// Load settings
async function loadSettings() {
  let playerActive = false;
  try {
    const s = await invoke("get_settings");
    if (document.getElementById("client-id"))
      document.getElementById("client-id").value = s.client_id || "";

    playerActive = !!s.spotify_player_active;
    // macOS-only capture-permission section reflects live auth state when
    // the user opens Settings. On Windows the section is always hidden.
    await checkCaptureStatus();
    document.getElementById("buf-sec").value = s.buffer_seconds;
    document.getElementById("capture-source-field").hidden = !s.source_selection;
    document.getElementById("capture-source").value = s.system_capture ? "system" : "spotify";
    document.getElementById("auth-state").textContent = s.authenticated ? "Connected ✓" : "Not connected";
    document.getElementById("hk-capture").value = s.capture_hotkey;
    document.getElementById("btn-seed").style.display = s.debug ? "" : "none";
    const seedEmpty = document.getElementById("btn-seed-empty");
    if (seedEmpty) seedEmpty.style.display = s.debug ? "inline-flex" : "none";
    document.getElementById("app-version").textContent = `Samplify v${s.version || "dev"}`;
  } catch (e) {
    console.error("Failed to load settings:", e);
  }
  renderStemModelStatus();
  // Warn up-front if the incompatible Microsoft Store Spotify is installed — Spicetify
  // (and therefore the in-app integration) can only patch the standalone spotify.com build.
  try {
    const kind = await invoke("spotify_install_kind");
    const el = document.getElementById("spicetify-status");
    const btn = document.getElementById("btn-spicetify");
    if (playerActive) {
      el.textContent = "Receiving track details from the Spotify playbar.";
      el.className = "auth-status";
      if (btn) btn.disabled = kind === "macos";
    } else if (kind === "macos") {
      el.textContent = "Install samplify.js through Spicetify on this Mac, then open Spotify.";
      if (btn) btn.disabled = true;
    } else if (kind === "store") {
      el.textContent = "⚠ Microsoft Store Spotify detected — not compatible. Uninstall it and install the standalone app from spotify.com.";
      el.className = "auth-status warn";
      if (btn) btn.disabled = true;
    } else if (kind === "none") {
      el.textContent = "Spotify not detected — install the standalone app from spotify.com first.";
      el.className = "auth-status warn";
      if (btn) btn.disabled = false;
    } else {
      el.textContent = "Waiting for track details. Open Spotify and select a track.";
      el.className = "auth-status";
      if (btn) btn.disabled = false;
    }
  } catch (e) { console.error("spotify_install_kind failed:", e); }
}

// Hotkey recording setup
function setupHotkeyRecorder(inputEl) {
  inputEl.addEventListener("keydown", (e) => {
    e.preventDefault();
    e.stopPropagation();

    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) {
      return;
    }

    const parts = [];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey) parts.push("Shift");
    if (e.metaKey) parts.push("Command");

    let keyName = e.key;
    if (keyName === " ") keyName = "Space";
    else if (keyName.length === 1) keyName = keyName.toUpperCase();

    parts.push(keyName);
    inputEl.value = parts.join("+");
  });
}

setupHotkeyRecorder(document.getElementById("hk-capture"));

document.getElementById("btn-spicetify").onclick = async () => {
  const btn = document.getElementById("btn-spicetify");
  const status = document.getElementById("spicetify-status");
  const orig = btn.textContent;
  btn.disabled = true;
  btn.textContent = "Installing…";
  status.style.color = "var(--muted)";
  status.textContent = "Installing Spicetify + extension and patching Spotify… (~30s, Spotify will restart)";
  try {
    const msg = await invoke("setup_spotify_integration");
    status.style.color = "var(--accent)";
    status.textContent = "✓ " + msg;
  } catch (e) {
    status.style.color = "#e66";
    status.textContent = "✗ " + e;
  } finally {
    btn.disabled = false;
    btn.textContent = orig;
  }
};

document.getElementById("save-cid").onclick = async () => {
  await invoke("save_client_id", { clientId: document.getElementById("client-id").value.trim() });
  showToast("Client ID saved");
  loadSettings();
};

document.getElementById("connect").onclick = async (e) => {
  const btn = e.currentTarget;
  if (btn.disabled) return;
  // Disable while the browser login is open so a second click can't start a
  // competing flow (which would fight over the callback port → false CSRF error).
  btn.disabled = true;
  const label = btn.textContent;
  btn.textContent = "Waiting for browser…";
  try {
    await invoke("begin_auth");
    showToast("Connected to Spotify");
    document.getElementById("auth-state").textContent = "Connected ✓";
  } catch (err) {
    showToast(`Auth failed: ${err}`);
  } finally {
    btn.disabled = false;
    btn.textContent = label;
  }
};

document.getElementById("save-buffer").onclick = async () => {
  try {
    await invoke("save_settings", {
      bufferSeconds: Number(document.getElementById("buf-sec").value),
      systemCapture: document.getElementById("capture-source").value === "system",
    });
    showToast("Capture settings saved");
  } catch (e) { showToast(String(e)); }
};

document.getElementById("save-hotkeys").onclick = async () => {
  const errorEl = document.getElementById("hotkeys-error");
  errorEl.style.display = "none";
  try {
    await invoke("set_hotkeys", {
      captureHotkey: document.getElementById("hk-capture").value,
    });
    showToast("Hotkey saved");
    initHotkeyHint();
  } catch (e) {
    errorEl.textContent = e;
    errorEl.style.display = "block";
    initHotkeyHint();
  }
};

document.getElementById("tb-capture").onclick = async () => {
  try {
    await invoke("trigger_capture_cmd");
  } catch (e) {
    showToast(`Capture failed: ${e}`);
  }
};

document.getElementById("btn-seed").onclick = async () => {
  try {
    await invoke("seed_dev_data");
    showToast("Dev data seeded successfully");
    navigateTo("library");
  } catch (e) {
    showToast(`Seeding failed: ${e}`);
  }
};

const seedEmptyBtn = document.getElementById("btn-seed-empty");
if (seedEmptyBtn) {
  seedEmptyBtn.onclick = async () => {
    try {
      await invoke("seed_dev_data");
      showToast("Demo samples loaded ✓");
      await renderLibrary();
    } catch (e) {
      showToast(`Seeding failed: ${e}`);
    }
  };
}

// Render tag view list with rich studio cards
function renderTagsView() {
  const grid = document.getElementById("tags-grid");
  if (!grid) return;
  grid.innerHTML = "";
  TAGS.forEach((t, idx) => {
    const el = document.createElement("div");
    el.className = "tag-list-item";
    el.style.cursor = "pointer";
    el.style.setProperty("--tag-c", t.color);
    
    // Generate mini soundwave SVG bars
    const barHeights = [20, 45, 75, 35, 90, 60, 100, 70, 40, 85, 55, 30];
    const barsSvg = barHeights.map((h, i) => 
      `<rect x="${i * 7}" y="${24 - (h * 0.22)}" width="4" height="${Math.max(4, h * 0.22)}" rx="2" fill="${t.color}" opacity="${0.4 + (h / 180)}"/>`
    ).join("");

    el.innerHTML = `
      <div class="tag-card-head">
        <span class="tag-dot" style="background:${t.color}; box-shadow: 0 0 8px ${t.color}"></span>
        <span class="tag-card-name">${t.name}</span>
        <span class="tag-card-key">[${idx + 1}]</span>
      </div>
      <div class="tag-card-wave">
        <svg viewBox="0 0 84 26" width="84" height="26">${barsSvg}</svg>
      </div>
    `;
    el.onclick = () => {
      activeTagFilter = t.name;
      navigateTo("library"); // renders the filtered list
    };
    grid.appendChild(el);
  });
}

// ---- Collections ----
async function renderCollections() {
  const listEl = document.getElementById("collections-list");
  listEl.innerHTML = "";
  let collections = [];
  try { collections = (await invoke("list_collections")) || []; }
  catch (e) { showToast(`Failed to load collections: ${e}`); return; }
  if (!collections.length) {
    listEl.innerHTML = `<p class="empty-hint">No collections yet. Create one above, then add clips from a note's Collections row.</p>`;
    return;
  }
  collections.forEach(col => {
    const card = document.createElement("div");
    card.className = "collection-card";
    card.innerHTML = `
      <div class="collection-head">
        <span class="collection-drag" title="Drag this whole pack into your DAW">⠿</span>
        <div class="collection-meta">
          <span class="collection-name">${escapeText(col.name)}</span>
          <span class="collection-count">${col.clip_count} clip${col.clip_count === 1 ? "" : "s"}</span>
        </div>
        <button class="btn secondary collection-view-btn">View</button>
        <button class="collection-del" title="Delete collection (keeps the clips)">🗑</button>
      </div>
      <div class="collection-clips" style="display:none"></div>`;

    card.querySelector(".collection-drag").addEventListener("mousedown", async (e) => {
      e.preventDefault(); e.stopPropagation();
      try {
        const paths = await invoke("prepare_collection_export", { collectionId: col.id });
        await startPathsDrag(paths);
      } catch (err) { showToast(`Drag failed: ${err}`); }
    });

    const clipsEl = card.querySelector(".collection-clips");
    card.querySelector(".collection-view-btn").onclick = async () => {
      if (clipsEl.style.display !== "none") { clipsEl.style.display = "none"; return; }
      clipsEl.style.display = "block";
      clipsEl.innerHTML = "Loading…";
      try {
        const clips = await invoke("get_collection_clips", { collectionId: col.id });
        clipsEl.innerHTML = clips.length ? "" : "<p class='empty-hint'>No clips in this collection.</p>";
        clips.forEach(c => {
          const len = (c.trim_start_ms != null && c.trim_end_ms != null) ? (c.trim_end_ms - c.trim_start_ms) : (c.duration_ms || 0);
          const row = document.createElement("div");
          row.className = "collection-clip-row";
          row.innerHTML = `<span class="cc-title">${escapeText(c.title)}</span><span class="cc-note">${escapeText(c.note_text || "")}</span><span class="cc-len">${fmtTime(len)}</span><button class="cc-remove" title="Remove from collection">✕</button>`;
          row.querySelector(".cc-remove").onclick = async () => {
            await invoke("set_mark_in_collection", { collectionId: col.id, markId: c.mark_id, member: false });
            renderCollections();
          };
          clipsEl.appendChild(row);
        });
      } catch (err) { clipsEl.innerHTML = `<p class='empty-hint'>Failed: ${escapeText(err)}</p>`; }
    };

    card.querySelector(".collection-del").onclick = async () => {
      if (confirm(`Delete collection "${col.name}"? The clips themselves are kept.`)) {
        await invoke("delete_collection", { id: col.id });
        renderCollections();
      }
    };
    listEl.appendChild(card);
  });
}

document.getElementById("btn-new-collection").onclick = async () => {
  const input = document.getElementById("new-collection-name");
  const name = input.value.trim();
  if (!name) return;
  try { await invoke("create_collection", { name }); input.value = ""; renderCollections(); }
  catch (e) { showToast(`${e}`); }
};
document.getElementById("new-collection-name").addEventListener("keydown", (e) => {
  if (e.key === "Enter") document.getElementById("btn-new-collection").click();
});

// Populate the expanded note editor's collection toggle-chips.
async function loadEditorCollections(markId, container) {
  container = container || document.getElementById(`editor-collections-${markId}`);
  if (!container) return;
  try {
    const [collections, memberOf] = await Promise.all([
      invoke("list_collections"),
      invoke("collections_for_mark", { markId }),
    ]);
    const memberSet = new Set(memberOf);
    container.innerHTML = "";
    collections.forEach(col => {
      const chip = document.createElement("span");
      chip.className = "ecol" + (memberSet.has(col.id) ? " on" : "");
      chip.textContent = col.name;
      chip.onclick = async (e) => {
        e.stopPropagation();
        const nowMember = !chip.classList.contains("on");
        try {
          await invoke("set_mark_in_collection", { collectionId: col.id, markId, member: nowMember });
          chip.classList.toggle("on", nowMember);
        } catch (err) { showToast(`${err}`); }
      };
      container.appendChild(chip);
    });
    const add = document.createElement("span");
    add.className = "ecol ecol-new";
    add.textContent = "+ New";
    add.onclick = async (e) => {
      e.stopPropagation();
      const name = prompt("New collection name:");
      if (!name || !name.trim()) return;
      try {
        const id = await invoke("create_collection", { name: name.trim() });
        await invoke("set_mark_in_collection", { collectionId: id, markId, member: true });
        loadEditorCollections(markId);
      } catch (err) { showToast(`${err}`); }
    };
    container.appendChild(add);
  } catch (e) { container.textContent = "Failed to load collections"; }
}

// Render Library Track list
async function renderLibrary(query = "") {
  try {
    let tracks = [];
    if (query.trim()) {
      activeTagFilter = null; // a text search overrides any tag filter
      const marks = await invoke("search_marks", { query });
      const tracksMap = {};
      marks.forEach(m => {
        if (!tracksMap[m.track_id]) {
          tracksMap[m.track_id] = {
            track_id: m.track_id,
            title: m.title,
            artist: m.artist,
            album_art_url: null,
            mark_count: 0,
            latest_mark_at: m.created_at,
          };
        }
        tracksMap[m.track_id].mark_count++;
      });
      tracks = Object.values(tracksMap);
    } else if (activeTagFilter) {
      tracks = await invoke("get_tracks_by_tag", { tag: activeTagFilter });
    } else {
      tracks = await invoke("get_library");
    }

    // Sample-list title reflects an active tag filter (click the ✕ to clear).
    const titleEl = document.getElementById("sample-list-title");
    if (activeTagFilter && !query.trim()) {
      titleEl.innerHTML = `Tag: ${escapeText(activeTagFilter)} <span class="clear-filter" title="Clear filter">✕</span>`;
      titleEl.style.cursor = "pointer";
      titleEl.onclick = () => { activeTagFilter = null; renderLibrary(); };
    } else {
      titleEl.textContent = "All Samples";
      titleEl.style.cursor = "";
      titleEl.onclick = null;
    }

    tracks = sortTracks(tracks);
    tracksListEl.innerHTML = "";
    sampleCountEl.textContent = tracks.length;

    if (tracks.length === 0) {
      detailEmptyEl.style.display = "flex";
      detailContentEl.style.display = "none";
      return;
    }

    tracks.forEach(t => {
      const el = document.createElement("div");
      el.dataset.trackId = String(t.track_id);
      el.className = `track-item ${selectedTrackId === t.track_id ? "active" : ""}`;
      el.onclick = () => selectTrack(t.track_id);
      
      const artSrc = t.album_art_url || "";
      el.innerHTML = `
        <div class="track-art">${icon("music", 18)}${artSrc ? `<img src="${escapeText(artSrc)}" alt="" onerror="this.remove()" />` : ""}</div>
        <div class="track-details">
          <div class="track-name">${t.favorite ? '<span class="fav-dot">★</span> ' : ""}${escapeText(t.title)}</div>
          <div class="track-artist">${escapeText(t.artist)}</div>
        </div>
        <div class="note-badge">${t.mark_count}</div>
      `;
      tracksListEl.appendChild(el);
    });

    // Background saves refresh the sidebar while the musician stays in Playground.
    if (views.library.style.display === "none") return;
    if (selectedTrackId && tracks.some(t => t.track_id === selectedTrackId)) {
      selectTrack(selectedTrackId);
    } else if (tracks.length > 0) {
      selectTrack(tracks[0].track_id);
    }
  } catch (e) {
    console.error("Failed to render library:", e);
  }
}

searchEl.addEventListener("input", (e) => renderLibrary(e.target.value));

// Select Track
async function selectTrack(trackId) {
  // Clicking a track from Collections/Tags/Settings should jump back to the library.
  if (views.library.style.display === "none") {
    Object.keys(views).forEach(k => { views[k].style.display = k === "library" ? "block" : "none"; });
    document.querySelectorAll(".nav-item").forEach(el => el.classList.toggle("active", el.getAttribute("data-view") === "library"));
  }
  selectedTrackId = trackId;
  document.querySelectorAll(".track-item").forEach((el) => {
    el.classList.toggle("active", Number(el.dataset.trackId) === Number(trackId));
  });

  try {
    const detail = await invoke("get_track_detail", { trackId });
    currentTrackDetail = detail;
    if (isStemMixerOpen() && !detail.marks.some(m => m.id === stemMixerSourceId())) closeStemMixer();

    detailEmptyEl.style.display = "none";
    detailContentEl.style.display = "block";

    // Set Header
    detailTrackTitleEl.textContent = detail.track.title;
    favStarEl.classList.toggle("active", !!detail.track.favorite);
    
    // Prefer the capture holding the selected note; otherwise the newest. Without
    // this, opening a track always jumped to the newest capture regardless of
    // which note the user had selected.
    const selMarkForSession = detail.marks.find(m => m.id === selectedMarkId);
    // Stems and bounced mixes get their own sessions, but they are not captures:
    // falling back to "the newest session" put a stem's 8s file in the header
    // waveform. Default to the newest real recording instead.
    const derivedSessionIds = new Set(detail.marks.filter(m => m.stem).map(m => m.session_id));
    const captures = detail.sessions.filter(s => !derivedSessionIds.has(s.id));
    let session = detail.sessions.find(s => s.id === selMarkForSession?.session_id)
      || captures[captures.length - 1]
      || detail.sessions[detail.sessions.length - 1];
    currentSession = session;
    clipDurationMs = session ? (session.duration_ms || 0) : 0;
    // Default the waveform's trim/loop handles to the selected note, or the first
    // note on this clip — so you can adjust and loop the region straight from the
    // library waveform without having to expand a note first.
    let selMark = detail.marks.find(m => m.id === selectedMarkId && m.session_id === session?.id);
    if (!selMark && session) selMark = detail.marks.find(m => m.session_id === session.id) || detail.marks[0];
    setActiveTrimForMark(session ? selMark : null);

    // Set timeline metadata: bpm/key if present from the session's marks
    const bpm = detail.marks.find(m => m.bpm)?.bpm || "—";
    const key = detail.marks.find(m => m.key)?.key || "—";
    detailTrackMetaEl.textContent = `${detail.track.artist} • Key: ${key} • BPM: ${bpm}`;

    // Update bottom transport bar metadata so it displays the active track
    tbTitleEl.textContent = detail.track.title;
    tbArtistEl.textContent = detail.track.artist;
    tbArtEl.onerror = () => { tbArtEl.style.display = "none"; };
    tbArtEl.src = detail.track.album_art_url || "";
    tbArtEl.style.display = detail.track.album_art_url ? "block" : "none";
    if (session) {
      const rebound = bindAudioToSession(session);
      const durSec = (session.duration_ms || 0) / 1000;
      tbDurationEl.textContent = durSec ? fmtSecs(durSec) : "0:00";
      tbCurrentTimeEl.textContent = rebound ? "0:00" : fmtSecs(audioEl.currentTime || 0);
    }

    // Render Waveform
    if (session) {
      waveformCanvas.style.display = "block";
      waveformPinsEl.style.display = "block";
      waveformPlaceholder.style.display = "none";
      // Label the timeline in SONG time (where the clip sits in the track), so a
      // clip at 2:40 with a 20s length reads "2:40 ... 3:00", not "0:00 ... 0:20".
      const clipStartMs = session.file_start_track_ms || 0;
      timelineStartEl.textContent = fmtTime(clipStartMs);
      timelineEndEl.textContent = fmtTime(clipStartMs + (session.duration_ms || 0));
      drawWaveform(session.id);
      renderPins(detail.marks, session);
      renderCaptureStrip();
    } else {
      waveformCanvas.style.display = "none";
      waveformPinsEl.style.display = "none";
      waveformPlaceholder.style.display = "block";
      timelineStartEl.textContent = "0:00";
      timelineEndEl.textContent = "0:00";
      captureStripEl.style.display = "none";
    }

    // Render Notes
    renderNotes(detail.marks);

  } catch (e) {
    console.error("Failed to load track detail:", e);
  }
}

// Waveform draw
let wfPeaks = null;
let clipDurationMs = 0;
let activeTrimMarkId = null;
let libLoop = false;
let activeAudioSessionId = null;
let playToken = 0;

function seekAudioTo(sec) {
  try {
    const dur = audioEl.duration;
    if (Number.isFinite(dur) && dur > 0) {
      audioEl.currentTime = Math.min(Math.max(0, sec), Math.max(0, dur - 0.01));
    } else if (Number.isFinite(sec) && sec >= 0) {
      audioEl.currentTime = sec;
    }
  } catch (e) {
    console.warn("Could not set currentTime:", e);
  }
}

function whenAudioReady(token, fn) {
  if (audioEl.readyState >= 1 && Number.isFinite(audioEl.duration) && audioEl.duration > 0) {
    fn();
    return;
  }
  const onLoaded = () => {
    audioEl.removeEventListener("error", onError);
    if (token === playToken) fn();
  };
  const onError = () => {
    audioEl.removeEventListener("loadedmetadata", onLoaded);
    if (token === playToken) showToast("Audio clip could not be loaded");
  };
  audioEl.addEventListener("loadedmetadata", onLoaded, { once: true });
  audioEl.addEventListener("error", onError, { once: true });
}

const waveform = new WaveformSelector(waveformCanvas, waveformCanvas.parentElement, {
  getPeaks: () => wfPeaks || [],
  getDurationMs: () => clipDurationMs,
  getPlayheadMs: () => {
    if (activeAudioSessionId === currentSession?.id && audioEl.duration) {
      return audioEl.currentTime * 1000;
    }
    return null;
  },
  isPlaying: () => activeAudioSessionId === currentSession?.id && !audioEl.paused,
  isLooping: () => libLoop,
  minSelMs: 100,
  allowMiddleDrag: false,
  allowPan: false,
  canDraw: () => currentSession != null,
  renderStyle: 'library',
  onSelectionDragMs: (startMs, endMs) => {
    updateTrimReadout(startMs, endMs);
    refreshSelectionBar();
  },
  onSelectionChange: (startMs, endMs) => {
    // Deliberately does NOT write. Persisting on mouse-up meant one capture
    // could only ever hold one loop — dragging clobbered the active note's trim.
    updateTrimReadout(startMs, endMs);
    refreshSelectionBar();
  },
  onSeek: (ms) => {
    const currentToken = ++playToken;
    if (currentSession && activeAudioSessionId !== currentSession.id) {
      bindAudioToSession(currentSession);
    }
    whenAudioReady(currentToken, () => {
      if (currentToken !== playToken) return;
      seekAudioTo(ms / 1000);
      if (audioEl.paused) {
        audioEl.play().then(() => {
          if (currentToken !== playToken) {
            audioEl.pause();
            return;
          }
          setTbPlaying(true);
          updatePlayStates();
        }).catch(err => {
          if (err.name === "AbortError") return;
          showToast(`Playback failed: ${err.message}`);
        });
      }
    });
  }
});

function renderWave() {
  waveform.draw();
}

function fitView() {
  waveform.fit();
}

function zoomView(factor, centerMs) {
  waveform.zoomView(factor, centerMs);
}

async function drawWaveform(sessionId) {
  try {
    wfPeaks = await invoke("get_waveform", { sessionId, buckets: 1000 });
    setSamplePeaks(wfPeaks);
    waveform.setView(0, clipDurationMs);
    waveform.draw();
  } catch (e) {
    console.error("Failed to render waveform:", e);
  }
}

function updateTrimReadout(startMs, endMs) {
  if (activeTrimMarkId == null) return;
  const el = document.getElementById(`trim-readout-${activeTrimMarkId}`);
  if (el) {
    el.textContent = `In ${fmtSecs(startMs / 1000)} · Out ${fmtSecs(endMs / 1000)} · ${fmtSecs((endMs - startMs) / 1000)}`;
  }
}

// The selection bar is the ONLY path that writes a trim. Dragging the waveform
// changes nothing on disk — it just arms these buttons.
function refreshSelectionBar() {
  const sel = waveform.getSelection();
  if (!sel || !currentSession) { selBarEl.style.display = "none"; return; }

  const mark = currentTrackDetail?.marks.find(m => m.id === activeTrimMarkId) || null;
  const savedStart = mark && mark.trim_start_ms != null ? mark.trim_start_ms : null;
  const savedEnd = mark && mark.trim_end_ms != null ? mark.trim_end_ms : null;
  const unchanged = savedStart != null
    && Math.abs(sel.startMs - savedStart) < 2
    && Math.abs(sel.endMs - savedEnd) < 2;
  if (unchanged) { selBarEl.style.display = "none"; return; }

  selBarEl.style.display = "flex";
  selBarReadoutEl.textContent =
    `In ${fmtSecs(sel.startMs / 1000)} · Out ${fmtSecs(sel.endMs / 1000)} · ${fmtSecs((sel.endMs - sel.startMs) / 1000)}`;
  selBarUpdateEl.disabled = activeTrimMarkId == null;
}

// One chip per capture of the current track — the only way to reach a capture
// that has no notes left on it. Hidden for single-capture tracks.
function renderCaptureStrip() {
  // Stems live on their own sessions so they inherit playback/drag/export for free,
  // but they are NOT captures — showing them here turned one chip into five, all
  // reading the same time. The strip lists only real recordings.
  const stemSessionIds = new Set(
    (currentTrackDetail?.marks || []).filter(m => m.stem).map(m => m.session_id)
  );
  const sessions = (currentTrackDetail?.sessions || []).filter(s => !stemSessionIds.has(s.id));
  captureStripEl.innerHTML = "";
  if (sessions.length < 2) { captureStripEl.style.display = "none"; return; }
  captureStripEl.style.display = "flex";
  sessions.forEach(s => {
    const notes = (currentTrackDetail.marks || []).filter(m => m.session_id === s.id).length;
    const chip = document.createElement("button");
    chip.className = "capture-chip" + (currentSession && s.id === currentSession.id ? " active" : "");
    chip.textContent = `${fmtTime(s.file_start_track_ms || 0)} · ${fmtSecs((s.duration_ms || 0) / 1000)} · ${notes} note${notes === 1 ? "" : "s"}`;
    chip.onclick = async () => {
      await showSession(s);
      // Point the trim handles at a note on this capture, if it has one.
      const m = (currentTrackDetail.marks || []).find(mm => mm.session_id === s.id) || null;
      setActiveTrimForMark(m);
      renderPins(currentTrackDetail.marks, s);
      renderWave();
    };
    captureStripEl.appendChild(chip);
  });
}

function renderPins(_marks, _session) {
  waveformPinsEl.innerHTML = "";
}

// Drag-out to DAW: a generated waveform icon + a helper that starts a native OS
// file drag of a clip (Copy mode, so the library keeps its file).
let _dragIcon = null;
function dragIcon() {
  if (_dragIcon) return _dragIcon;
  const c = document.createElement("canvas"); c.width = 72; c.height = 56;
  const x = c.getContext("2d");
  x.fillStyle = "#161619"; x.fillRect(0, 0, 72, 56);
  x.strokeStyle = "#E8912D"; x.lineWidth = 1; x.strokeRect(0.5, 0.5, 71, 55);
  x.fillStyle = "#E8912D";
  for (let i = 0; i < 22; i++) { const h = 6 + Math.abs(Math.sin(i * 0.8)) * 38; x.fillRect(6 + i * 2.7, 28 - h / 2, 2, h); }
  _dragIcon = c.toDataURL("image/png");
  return _dragIcon;
}
async function startPathsDrag(paths) {
  if (!paths || !paths.length) return;
  const core = window.__TAURI__.core;
  await core.invoke("plugin:drag|start_drag", {
    item: paths,
    image: dragIcon(),
    options: { mode: "copy" },
    onEvent: new core.Channel(),
  });
}
async function startClipDrag(filePath) {
  if (!filePath) return;
  try { await startPathsDrag([filePath]); }
  catch (err) { showToast(`Drag failed: ${err}`); }
}

// Render Notes
function renderNotes(allMarks) {
  notesListEl.innerHTML = "";
  // Raw stems live in the mixer, not the list: four near-identical rows per split
  // buried the notes they came from. A saved mix is an ordinary note and stays.
  const stemSources = new Set(allMarks.filter(m => RAW_STEMS.includes(m.stem)).map(m => m.source_mark_id));
  const marks = allMarks.filter(m => !RAW_STEMS.includes(m.stem));
  noteCountTitleEl.textContent = marks.length;

  const sessDur = {};
  const sessPath = {};
  const sessStart = {};
  (currentTrackDetail?.sessions || []).forEach(s => {
    sessDur[s.id] = s.duration_ms || 0;
    sessPath[s.id] = s.file_path;
    sessStart[s.id] = s.file_start_track_ms || 0;
  });

  marks.forEach(mark => {
    const card = document.createElement("div");
    card.id = `note-card-${mark.id}`;
    card.className = `note-card ${selectedMarkId === mark.id ? "expanded" : ""}`;

    const tagsHtml = (mark.tags || []).map(t => {
      const color = (TAGS.find(tag => tag.name === t)?.color) || "var(--muted)";
      return `<span class="tag-chip" style="color:${color}">${escapeText(t)}</span>`;
    }).join("");

    // Length and song position come from the mark's OWN trim, not the capture's
    // full duration. Several notes can share one capture now, so using the session
    // length rendered them all identically — same numbers, different audio.
    // Derived from the session start rather than mark.track_ms because the two
    // creation paths disagree: a captured mark stores the session start and gains a
    // trim later, while create_mark_from_selection folds the trim in already.
    const sessLenMs = sessDur[mark.session_id] || 0;
    const inMs = mark.trim_start_ms != null ? mark.trim_start_ms : (mark.session_offset_ms || 0);
    const clipLenMs = (mark.trim_start_ms != null && mark.trim_end_ms != null)
      ? (mark.trim_end_ms - mark.trim_start_ms)
      : sessLenMs;
    const songStart = (sessStart[mark.session_id] || 0) + inMs;
    const songEnd = songStart + clipLenMs;

    const editorTags = TAGS.map(t =>
      `<span class="etag ${(mark.tags || []).includes(t.name) ? "on" : ""}" style="color:${t.color}" data-name="${t.name}">${t.name}</span>`
    ).join("");

    card.innerHTML = `
      <div class="note-row">
        <button class="btn-play-note" id="btn-play-note-${mark.id}" title="Play">${icon("play", 12)}</button>
        <div class="note-timing">
          <span class="note-len">${clipLenMs ? fmtTime(clipLenMs) : "—"}</span>
          <span class="note-songpos">${fmtTime(songStart)}–${fmtTime(songEnd)}</span>
        </div>
        <span class="note-text" id="note-text-label-${mark.id}">${escapeText(mark.note_text) || "<em>(no note)</em>"}${mark.stem === "mix" ? ` <span class="stem-badge" title="Bounced from the stem mixer">mix</span>` : ""}</span>
        <div class="note-tags">${tagsHtml}</div>
        <div class="note-actions">
          ${stemSources.has(mark.id)
            ? `<button class="stems-chip" title="Open the stem mixer">${icon("stems", 13)} Stems</button>`
            : (mark.stem === "mix" ? "" : `<button class="icon-btn btn-stem-split" title="${stemModel.present ? "Split into stems (drums / bass / other / vocals)" : "Stem separation model not installed"}" ${stemModel.present ? "" : "disabled"}>${icon("stems", 15)}</button>`)}
          <button class="icon-btn btn-pg-add" title="Add to Playground">${icon("grid", 15)}</button>
          <span class="icon-btn note-drag" title="Drag into your DAW">${icon("grip", 15)}</span>
          <button class="icon-btn btn-more" title="Details">${icon("more", 15)}</button>
        </div>
      </div>
      <div class="editor-layout" style="display: ${selectedMarkId === mark.id ? "flex" : "none"}">
        <div class="editor-row">
          <div class="editor-field">
            <label>Rating</label>
            <div class="rating-stars">
              ${[1,2,3,4,5].map(n => `<span class="rating-star ${n <= (mark.rating || 0) ? "active" : ""}" data-star="${n}">★</span>`).join("")}
            </div>
          </div>
          <div class="editor-field">
            <label>Key</label>
            <input class="editor-input small" id="edit-key-${mark.id}" value="${escapeText(mark.key)}" placeholder="Key" />
          </div>
          <div class="editor-field">
            <label>BPM</label>
            <input class="editor-input small" type="number" id="edit-bpm-${mark.id}" value="${mark.bpm || ""}" placeholder="BPM" />
          </div>
        </div>
        <div class="editor-field">
          <label>Tags</label>
          <div class="editor-tags" id="editor-tags-${mark.id}">${editorTags}</div>
        </div>
        <div class="editor-field">
          <label>Trim <span class="trim-hint">— drag the handles on the waveform</span></label>
          <div class="trim-info">
            <span id="trim-readout-${mark.id}">${trimReadoutText(mark)}</span>
            <button class="btn trim-reset-btn">Reset to full clip</button>
          </div>
        </div>
        <div class="editor-field">
          <label>Collections <span class="trim-hint">— click to add this clip to a pack</span></label>
          <div class="editor-collections" id="editor-collections-${mark.id}"></div>
        </div>
        <textarea class="editor-textarea" id="edit-textarea-${mark.id}" placeholder="Note details… (saves automatically)">${escapeText(mark.note_text || "")}</textarea>
        <div class="editor-actions">
          <span class="autosave-hint">Notes, rating &amp; tags save automatically</span>
          ${stemSources.has(mark.id) ? `<button class="btn ghost resplit-btn" title="Run separation again (the current stems stay until you delete them)">Split again</button>` : ""}
          <button class="btn export-note-btn">${icon("export", 13)} Export clip</button>
          <button class="btn secondary delete-btn">Delete Note</button>
        </div>
      </div>
    `;

    // Note Play Trigger
    card.querySelector(`.btn-play-note`).onclick = (e) => {
      e.stopPropagation();
      if (currentPlayingMark && currentPlayingMark.id === mark.id && !audioEl.paused) {
        audioEl.pause();
        updatePlayStates();
      } else {
        playNote(mark);
      }
    };

    // Expand Note Card
    card.onclick = () => {
      selectNoteRow(mark.id);
    };

    // Split into stems. One job runs app-wide (each inference peaks near 4.9 GB),
    // so a second click while one is running is refused by the backend, not here.
    const splitBtn = card.querySelector(".btn-stem-split");
    if (splitBtn) splitBtn.onclick = (e) => { e.stopPropagation(); startStemSplit(mark); };
    const resplitBtn = card.querySelector(".resplit-btn");
    if (resplitBtn) resplitBtn.onclick = (e) => { e.stopPropagation(); startStemSplit(mark); };
    const stemsChip = card.querySelector(".stems-chip");
    if (stemsChip) stemsChip.onclick = (e) => { e.stopPropagation(); openStemMixer(mark.id); };

    // Send to Playground
    const pgBtn = card.querySelector(".btn-pg-add");
    if (pgBtn) {
      pgBtn.onclick = (e) => {
        e.stopPropagation();
        // Solo in the mixer means "this is the part I want". The note-row
        // playground button used to ignore that and send the full capture.
        if (isStemMixerOpen() && stemMixerSourceId() === mark.id) {
          const soloed = mixerSoloedPlaygroundClips();
          if (soloed.length) {
            for (const clip of soloed) playgroundAddClip(clip);
            return;
          }
        }
        const filePath = sessPath[mark.session_id];
        if (!filePath) { showToast("This note has no audio clip"); return; }
        playgroundAddClip({
          markId: mark.id,
          title: currentTrackDetail.track.title,
          artist: currentTrackDetail.track.artist,
          noteText: mark.note_text,
          filePath,
          trimStartMs: mark.trim_start_ms,
          trimEndMs: mark.trim_end_ms,
          tags: mark.tags,
          rating: mark.rating,
          key: mark.key,
          bpm: mark.bpm,
        });
      };
    }

    // Star Selection
    card.querySelectorAll(".rating-star").forEach(star => {
      star.onclick = async (e) => {
        e.stopPropagation();
        const rating = Number(star.getAttribute("data-star"));
        await saveMarkMeta(mark.id, { rating });
      };
    });

    // Save triggers on blur
    const keyInput = card.querySelector(`#edit-key-${mark.id}`);
    if (keyInput) {
      keyInput.onclick = e => e.stopPropagation();
      keyInput.onblur = () => { if (keyInput.value !== (mark.key || "")) saveMarkMeta(mark.id, { key: keyInput.value }); };
    }

    const bpmInput = card.querySelector(`#edit-bpm-${mark.id}`);
    if (bpmInput) {
      bpmInput.onclick = e => e.stopPropagation();
      bpmInput.onblur = () => { const v = bpmInput.value ? Number(bpmInput.value) : null; if (v !== (mark.bpm != null ? mark.bpm : null)) saveMarkMeta(mark.id, { bpm: v }); };
    }

    const textarea = card.querySelector(`#edit-textarea-${mark.id}`);
    if (textarea) {
      textarea.onclick = e => e.stopPropagation();
      textarea.onblur = () => { if (textarea.value !== (mark.note_text || "")) saveMarkMeta(mark.id, { noteText: textarea.value }); };
    }

    // Tag editing (toggle chips, save immediately)
    card.querySelectorAll(`#editor-tags-${mark.id} .etag`).forEach(chip => {
      chip.onclick = async (e) => {
        e.stopPropagation();
        const name = chip.dataset.name;
        const cur = new Set(mark.tags || []);
        if (cur.has(name)) cur.delete(name); else cur.add(name);
        await saveMarkMeta(mark.id, { tags: [...cur] });
      };
    });

    // Collection membership chips (only the expanded editor needs them)
    if (selectedMarkId === mark.id) loadEditorCollections(mark.id, card.querySelector(".editor-collections"));

    // Trim reset (clear the trim region back to the full clip)
    const trimReset = card.querySelector(".trim-reset-btn");
    if (trimReset) {
      trimReset.onclick = async (e) => {
        e.stopPropagation();
        try {
          await invoke("update_mark_trim", { markId: mark.id, trimStartMs: null, trimEndMs: null });
          mark.trim_start_ms = null; mark.trim_end_ms = null;
          if (activeTrimMarkId === mark.id) { waveform.clearSelection(); }
          const el = document.getElementById(`trim-readout-${mark.id}`);
          if (el) el.textContent = "Full clip";
          showToast("Trim reset to full clip");
        } catch (err) { showToast(`Reset failed: ${err}`); }
      };
    }

    // Drag-out to DAW (grab the handle and drop into a DAW / file explorer)
    const dragHandle = card.querySelector(".note-drag");
    if (dragHandle) {
      dragHandle.addEventListener("mousedown", async (e) => {
        e.stopPropagation();
        e.preventDefault();
        try {
          const path = await invoke("prepare_drag_wav", { markId: mark.id });
          await startPathsDrag([path]);
        } catch (err) {
          showToast(`Drag failed: ${err}`);
        }
      });
    }

    // Export trigger
    const exportBtn = card.querySelector(".export-note-btn");
    if (exportBtn) {
      exportBtn.onclick = (e) => { e.stopPropagation(); exportMark(mark.id); };
    }

    // Delete trigger
    const deleteBtn = card.querySelector(".delete-btn");
    if (deleteBtn) {
      deleteBtn.onclick = async (e) => {
        e.stopPropagation();
        if (confirm("Delete this note?")) {
          await invoke("delete_mark", { markId: mark.id });
          showToast("Note deleted");
          selectedMarkId = null;
          selectTrack(selectedTrackId);
        }
      };
    }

    notesListEl.appendChild(card);
  });
}

// Select Note Row (Expand details)
function setActiveTrimForMark(mark) {
  // Which mark the waveform selection edits (used when you drag to create one).
  activeTrimMarkId = mark && mark.session_id != null ? mark.id : null;
  // Only show a selection when a real trim exists — otherwise the waveform is clean
  // and you drag on it to draw one (matches the capture card; no stray edge handles).
  if (mark && mark.trim_start_ms != null && mark.trim_end_ms != null) {
    waveform.setSelection(mark.trim_start_ms, mark.trim_end_ms);
  } else {
    waveform.clearSelection();
  }
}

// Load a capture's waveform, timeline labels and peaks. Extracted from
// useMarkSession so the capture strip can switch captures without going
// through a mark — a capture whose notes were all deleted has no mark to click.
async function showSession(session) {
  if (!session) return null;
  if (!currentSession || currentSession.id !== session.id) {
    currentSession = session;
    clipDurationMs = session.duration_ms || 0;
    const clipStartMs = session.file_start_track_ms || 0;
    timelineStartEl.textContent = fmtTime(clipStartMs);
    timelineEndEl.textContent = fmtTime(clipStartMs + (session.duration_ms || 0));
    // The transport must follow the capture on screen. Without this, switching
    // captures via the strip changed the picture while <audio> kept playing
    // whichever file was loaded last — so the waveform showed one capture and
    // you heard another, and the playhead sat at a position meaningless to it.
    bindAudioToSession(session);
    await drawWaveform(session.id); // loads THIS session's peaks + resets the view
  }
  renderCaptureStrip();
  return session;
}

async function useMarkSession(mark) {
  const sessions = currentTrackDetail?.sessions || [];
  const session = sessions.find(s => s.id === mark?.session_id) || currentSession;
  return showSession(session);
}

async function selectNoteRow(markId) {
  if (selectedMarkId === markId) return;
  selectedMarkId = markId;
  const mark = currentTrackDetail.marks.find(m => m.id === markId);
  await useMarkSession(mark); // switch the waveform to this note's own clip
  setActiveTrimForMark(mark);
  renderNotes(currentTrackDetail.marks);
  renderWave();
}

// Save Mark Metadata
async function saveMarkMeta(markId, overrides) {
  const m = currentTrackDetail.marks.find(mark => mark.id === markId);
  if (!m) return;
  
  const noteText = overrides.noteText !== undefined ? overrides.noteText : m.note_text;
  const rating = overrides.rating !== undefined ? overrides.rating : m.rating;
  const key = overrides.key !== undefined ? overrides.key : m.key;
  const bpm = overrides.bpm !== undefined ? overrides.bpm : m.bpm;
  const tags = overrides.tags !== undefined ? overrides.tags : m.tags;

  try {
    await invoke("update_mark_meta", {
      markId,
      noteText,
      tags,
      rating,
      key,
      bpm,
    });
    // refresh details
    const detail = await invoke("get_track_detail", { trackId: selectedTrackId });
    currentTrackDetail = detail;
    renderNotes(detail.marks);
    showToast("Saved ✓");
  } catch (e) {
    showToast(`Failed to update metadata: ${e}`);
  }
}

// Handoff audio state
let currentPlayingMark = null;
let playTrimEndSec = null; // when set, playback pauses/loops at this clip position
let playLoopStartSec = 0;  // where loop playback restarts from

// Point the <audio> element at a capture, if it isn't already. Kept separate from
// playNote so the capture strip can switch captures without starting playback.
function bindAudioToSession(session) {
  if (!session || !session.file_path) return false;
  const target = window.__TAURI__.core.convertFileSrc(session.file_path);
  if (audioEl.src === target) {
    activeAudioSessionId = session.id;
    return false;
  }
  activeAudioSessionId = session.id;
  audioEl.pause();
  audioEl.src = target;
  audioEl.load();
  currentPlayingMark = null;
  updatePlayStates();
  return true;
}

// Play note audio clip (honors the mark's trim region if set)
async function playNote(mark) {
  const currentToken = ++playToken;

  // Point the transport + waveform at THIS mark's session (a track can have several
  // separate recordings, each its own file) so it plays its own clip — not whichever
  // session was selected last.
  const session = await useMarkSession(mark);
  if (currentToken !== playToken) return;
  if (!session) return;

  // Make the on-screen waveform + trim handles match the clip we're about to play.
  setActiveTrimForMark(mark);
  renderWave();

  // Trim wins over the raw session offset for where playback starts/stops.
  const startMs = mark.trim_start_ms != null ? mark.trim_start_ms : (mark.session_offset_ms || 0);
  playTrimEndSec = mark.trim_end_ms != null ? mark.trim_end_ms / 1000 : null;
  playLoopStartSec = startMs / 1000;

  // Set Bottom Transport Meta
  tbTitleEl.textContent = currentTrackDetail.track.title;
  tbArtistEl.textContent = currentTrackDetail.track.artist;
  tbArtEl.onerror = () => { tbArtEl.style.display = "none"; };
  tbArtEl.src = currentTrackDetail.track.album_art_url || "";
  tbArtEl.style.display = currentTrackDetail.track.album_art_url ? "block" : "none";

  // showSession() (via useMarkSession above) has already pointed the transport at
  // this capture; this is a no-op unless the src somehow drifted.
  bindAudioToSession(session);

  const startSec = Math.max(0, startMs / 1000);

  const executePlay = () => {
    if (currentToken !== playToken) return;
    seekAudioTo(startSec);
    audioEl.play().then(() => {
      if (currentToken !== playToken) {
        audioEl.pause();
        return;
      }
      setTbPlaying(true);
      currentPlayingMark = mark;
      updatePlayStates();
    }).catch(err => {
      if (err.name === "AbortError") return;
      console.error("Playback error:", err);
      showToast(`Playback failed: ${err.message}`);
    });
  };

  whenAudioReady(currentToken, executePlay);
}

function updatePlayStates() {
  document.querySelectorAll(".btn-play-note").forEach(btn => {
    const id = Number(btn.id.replace("btn-play-note-", ""));
    if (currentPlayingMark && currentPlayingMark.id === id && !audioEl.paused) {
      btn.innerHTML = icon("pause", 12);
      btn.classList.add("active");
    } else {
      btn.innerHTML = icon("play", 12);
      btn.classList.remove("active");
    }
  });
}

// Audio transport wiring
tbPlayPauseEl.onclick = () => {
  const currentToken = ++playToken;
  if (audioEl.paused) {
    if ((!audioEl.src || activeAudioSessionId !== currentSession?.id) && currentSession) {
      bindAudioToSession(currentSession);
    }
    if (audioEl.src) {
      whenAudioReady(currentToken, () => {
        if (currentToken !== playToken) return;
        audioEl.play().then(() => {
          if (currentToken !== playToken) {
            audioEl.pause();
            return;
          }
          setTbPlaying(true);
          updatePlayStates();
        }).catch(err => {
          if (err.name === "AbortError") return;
          showToast(`Playback failed: ${err.message}`);
        });
      });
    }
  } else {
    audioEl.pause();
    setTbPlaying(false);
    updatePlayStates();
  }
};

// Which region playback is confined to.
//
// The on-screen selection wins only when it belongs to the note actually playing,
// so dragging a loop still auditions live. Previously the selection always won,
// which meant pressing play on one note replayed whatever region was on screen —
// two notes with genuinely different trims sounded identical.
function loopBounds() {
  const sel = waveform.getSelection();
  const selOwnsPlayback = sel
    && (currentPlayingMark == null || currentPlayingMark.id === activeTrimMarkId);
  if (selOwnsPlayback) {
    return { startSec: sel.startMs / 1000, endSec: sel.endMs / 1000 };
  }
  return {
    startSec: playLoopStartSec || 0,
    endSec: playTrimEndSec != null
      ? playTrimEndSec
      : (libLoop && clipDurationMs > 0 ? clipDurationMs / 1000 : null),
  };
}

// Called from BOTH the ~4Hz timeupdate and the 60Hz playhead tick. Clamping only
// on timeupdate let the playhead visibly run up to a quarter-second past the loop
// edge before snapping back.
function enforceLoopBounds() {
  const { startSec, endSec } = loopBounds();
  if (endSec == null || !audioEl.duration || audioEl.paused) return;
  if (audioEl.currentTime >= endSec - 0.02) {
    if (libLoop) { audioEl.currentTime = startSec; }
    else { audioEl.pause(); updatePlayStates(); }
  }
}

audioEl.ontimeupdate = () => {
  enforceLoopBounds();
  tbCurrentTimeEl.textContent = fmtSecs(audioEl.currentTime);
  if (audioEl.duration) {
    tbDurationEl.textContent = fmtSecs(audioEl.duration);
    renderWave();
    updateVisualizerPlayhead(audioEl.currentTime, audioEl.duration);
  }
};

// Loop the whole clip when there's no trim end and Loop is on.
audioEl.addEventListener("ended", () => {
  if (libLoop) {
    audioEl.currentTime = playLoopStartSec;
    audioEl.play().catch(() => {});
    setTbPlaying(true);
    updatePlayStates();
  }
});

// Waveform toolbar: loop / zoom / fit.
const waveToolbarEl = document.querySelector(".wave-toolbar");
if (waveToolbarEl) {
  waveToolbarEl.addEventListener("mousedown", (e) => e.stopPropagation());
  waveToolbarEl.addEventListener("click", (e) => e.stopPropagation());
}
document.getElementById("wave-fit").onclick = (e) => { e.stopPropagation(); fitView(); };
document.getElementById("wave-zoom-in").onclick = (e) => { e.stopPropagation(); zoomView(1 / 1.5); };
document.getElementById("wave-zoom-out").onclick = (e) => { e.stopPropagation(); zoomView(1.5); };
document.getElementById("wave-loop").onclick = (e) => {
  e.stopPropagation();
  libLoop = !libLoop;
  document.getElementById("wave-loop").classList.toggle("active", libLoop);
};

selBarUpdateEl.onclick = async () => {
  const sel = waveform.getSelection();
  if (!sel || activeTrimMarkId == null) return;
  const markId = activeTrimMarkId;
  const ts = Math.round(sel.startMs);
  const te = Math.round(sel.endMs);
  try {
    await invoke("update_mark_trim", { markId, trimStartMs: ts, trimEndMs: te });
    const m = currentTrackDetail.marks.find(mm => mm.id === markId);
    if (m) { m.trim_start_ms = ts; m.trim_end_ms = te; }
    const readoutEl = document.getElementById(`trim-readout-${markId}`);
    if (readoutEl) readoutEl.textContent = `In ${fmtSecs(ts / 1000)} · Out ${fmtSecs(te / 1000)} · ${fmtSecs((te - ts) / 1000)}`;
    refreshSelectionBar();
    showToast("Note updated ✓");
  } catch (err) { showToast(`Trim save failed: ${err}`); }
};

selBarSaveNewEl.onclick = async () => {
  const sel = waveform.getSelection();
  if (!sel || !currentSession) return;
  if (sel.endMs - sel.startMs < 100) { showToast("Selection is too short"); return; }
  try {
    const newId = await invoke("create_mark_from_selection", {
      sessionId: currentSession.id,
      trimStartMs: Math.round(sel.startMs),
      trimEndMs: Math.round(sel.endMs),
      noteText: selBarNoteEl.value.trim(),
      tags: [],
    });
    selBarNoteEl.value = "";
    selectedMarkId = newId;
    await selectTrack(selectedTrackId);   // re-renders the notes list with the new note
    showToast("Saved as new note ✓");
  } catch (err) { showToast(`Save failed: ${err}`); }
};

selBarRevertEl.onclick = () => {
  const mark = currentTrackDetail?.marks.find(m => m.id === activeTrimMarkId) || null;
  if (mark && mark.trim_start_ms != null && mark.trim_end_ms != null) {
    waveform.setSelection(mark.trim_start_ms, mark.trim_end_ms);
  } else {
    waveform.clearSelection();
  }
  waveform.draw();
  refreshSelectionBar();
};

// Smooth playhead while playing (ontimeupdate alone is only ~4Hz).
function libPlayheadTick() {
  if (audioEl.paused || !audioEl.duration) return;
  enforceLoopBounds();   // at 60Hz, so the playhead never visibly leaves the loop
  renderWave();
  requestAnimationFrame(libPlayheadTick);
}
audioEl.addEventListener("play", () => requestAnimationFrame(libPlayheadTick));

audioEl.onloadedmetadata = () => {
  tbDurationEl.textContent = fmtSecs(audioEl.duration);
};

tbVolumeEl.oninput = (e) => {
  audioEl.volume = Number(e.target.value) / 100;
};

// Playback speed for auditioning — the browser time-stretches with pitch preserved.
const tempoSliderEl = document.getElementById("tb-tempo");
const tempoValEl = document.getElementById("tb-tempo-val");
function applyTempo() {
  const rate = Number(tempoSliderEl.value) / 100;
  audioEl.playbackRate = rate;
  audioEl.preservesPitch = true;
  audioEl.mozPreservesPitch = true;
  audioEl.webkitPreservesPitch = true;
  tempoValEl.textContent = rate.toFixed(2) + "×";
}
tempoSliderEl.oninput = applyTempo;
tempoSliderEl.ondblclick = () => { tempoSliderEl.value = 100; applyTempo(); };
// Re-apply on each play (loading a new clip can reset the rate).
audioEl.addEventListener("play", applyTempo);

// Favorite toggle
favStarEl.onclick = async () => {
  if (!currentTrackDetail) return;
  const newVal = !currentTrackDetail.track.favorite;
  try {
    await invoke("set_favorite", { trackId: selectedTrackId, favorite: newVal });
    currentTrackDetail.track.favorite = newVal;
    favStarEl.classList.toggle("active", newVal);
    renderLibrary(searchEl.value);
  } catch (e) { showToast(`Favorite failed: ${e}`); }
};

// Delete whole track (marks + sessions + clip files)
trackDeleteEl.onclick = async () => {
  if (!selectedTrackId || !currentTrackDetail) return;
  const name = currentTrackDetail.track.title;
  if (!confirm(`Delete "${name}" and all its clips? This removes the audio files from disk.`)) return;
  try {
    await invoke("delete_track", { trackId: selectedTrackId });
    showToast("Track deleted");
    selectedTrackId = null; selectedMarkId = null; currentTrackDetail = null;
    audioEl.pause();
    renderLibrary(searchEl.value);
    loadStorageInfo();
  } catch (e) { showToast(`Delete failed: ${e}`); }
};

// Sort control
sortSelectEl.onchange = () => { sortMode = sortSelectEl.value; renderLibrary(searchEl.value); };

// Export a mark's clip to the Exports folder (reveals in Explorer).
async function exportMark(markId) {
  try {
    const path = await invoke("export_mark", { markId });
    const name = path.split(/[\\/]/).pop();
    showToast(`Exported → ${name}`);
  } catch (e) { showToast(`Export failed: ${e}`); }
}
// Show a shortcut only when this process owns the binding. The stored
// string uses Tauri's `CommandOrControl+` modifier (mapped to Cmd on macOS,
// Ctrl on Windows) so we render the platform-correct label instead of the
// raw token — `Cmd+Shift+N` on Mac, `Ctrl+Shift+N` on Windows.
function displayHotkey(raw) {
  if (!raw) return raw;
  const mod = capturePlatform === "macos" ? "Cmd" : "Ctrl";
  return raw.replace(/^CommandOrControl\+/, mod + "+").replace(/^Ctrl\+/, "Ctrl+");
}
async function initHotkeyHint() {
  try {
    const s = await invoke("get_settings");
    const label = displayHotkey(s.capture_hotkey);
    const active = await invoke("plugin:global-shortcut|is_registered", { shortcut: s.capture_hotkey });
    document.getElementById("empty-hotkey").textContent = label;
    document.getElementById("empty-shortcut").hidden = !active;
    const message = `${label} is unavailable. Use Capture or choose another shortcut in Settings.`;
    for (const id of ["empty-shortcut-status", "hotkey-status"]) {
      const status = document.getElementById(id);
      status.textContent = active ? "" : message;
      status.hidden = active;
    }
    document.getElementById("tb-capture").title = active ? `Capture (${label})` : message;
  } catch {}
}

// Listeners
listen("mark-created", () => {
  renderLibrary(searchEl.value);
  loadStorageInfo();
});

listen("mark-error", (e) => {
  showToast(`Mark failed: ${e.payload}`);
});

// Setup
mountIcons();
loadStorageInfo();
checkCaptureStatus().then(initHotkeyHint); // platform-aware hotkey label needs capturePlatform set first
const visCanvas = document.getElementById("vis-canvas");
if (visCanvas) {
  initVisualizer(
    visCanvas,
    document.getElementById("vis-mode-label"),
    document.getElementById("vis-status-text")
  );
}

initPlayground({ showToast, onPlaybackStart: () => { audioEl.pause(); stemMixerPause(); } });
initStemMixer({
  showToast,
  playgroundAddClip,
  startPathsDrag,
  connectAudioSource,
  onPlaybackStateChange: (active) => setAudioActive(active),
  // One thing plays at a time: mixer taking over pauses the clip; clip play
  // pauses the mixer (audioEl "play" listener).
  onPlaybackStart: () => { stopPlayground(); if (!audioEl.paused) { audioEl.pause(); updatePlayStates(); } },
  onSetChanged: (newMarkId) => { if (newMarkId) selectedMarkId = newMarkId; if (selectedTrackId) selectTrack(selectedTrackId); },
});
loadStemModelStatus().then(() => { if (currentTrackDetail) renderNotes(currentTrackDetail.marks); });
navigateTo("library");

// ---- Stem separation: model status + the job card ----
async function loadStemModelStatus() {
  try { stemModel = await invoke("stem_model_status"); }
  catch (e) { stemModel = { present: false, path: "", bytes: 0, max_clip_seconds: 60 }; }
  renderStemModelStatus();
}

function renderStemModelStatus() {
  const el = document.getElementById("stem-model-status");
  if (!el) return;
  if (stemModel.present) {
    const mb = (stemModel.bytes / 1048576).toFixed(0);
    el.innerHTML = `<span class="model-ok">${icon("check", 13)} Model installed</span> · htdemucs · ${mb} MB · clips up to ${stemModel.max_clip_seconds}s<div class="model-path" title="${escapeText(stemModel.path)}">${escapeText(stemModel.path)}</div>`;
  } else {
    el.innerHTML = `<span class="model-missing">${icon("warning", 13)} Model not found</span> — reinstall Samplify, or run <code>npm run fetch-model</code> in a source checkout.<div class="model-path">${escapeText(stemModel.path)}</div>`;
  }
}

// Exactly one split runs at a time (each inference peaks near 4.9 GB); the card
// tracks it with real per-chunk progress from the backend's stem-progress event.
const stemJob = { markId: null, trackId: null, startedAt: 0 };
const jobEl = document.getElementById("stem-job");
const jobTitleEl = document.getElementById("stem-job-title");
const jobFillEl = document.getElementById("stem-job-fill");
const jobMetaEl = document.getElementById("stem-job-meta");

async function startStemSplit(mark) {
  if (stemJob.markId != null) { showToast("A stem split is already running"); return; }
  if (!stemModel.present) { showToast("Stem separation model not installed — see Settings"); return; }
  stemJob.markId = mark.id;
  stemJob.trackId = selectedTrackId;
  stemJob.startedAt = performance.now();
  jobTitleEl.textContent = `${currentTrackDetail?.track.title || "Note"} — ${mark.note_text || "(no note)"}`;
  jobFillEl.style.width = "0%";
  jobMetaEl.textContent = "Loading model…";
  jobEl.style.display = "block";
  try {
    await invoke("separate_mark_stems", { markId: mark.id });
    showToast("Stems ready ✓");
    if (selectedTrackId === stemJob.trackId) {
      await selectTrack(selectedTrackId);
      openStemMixer(mark.id);
    }
  } catch (err) {
    showToast(`${err}`);
  } finally {
    stemJob.markId = null;
    jobEl.style.display = "none";
  }
}

document.getElementById("stem-job-cancel").onclick = async () => {
  jobMetaEl.textContent = "Cancelling after this chunk…";
  try { await invoke("cancel_stem_separation"); } catch (e) { showToast(`${e}`); }
};

listen("stem-progress", (e) => {
  const p = e && e.payload ? e.payload : {};
  if (p.markId !== stemJob.markId || !p.total) return;
  const frac = p.done / p.total;
  const elapsed = (performance.now() - stemJob.startedAt) / 1000;
  const left = frac > 0 ? Math.max(0, elapsed / frac - elapsed) : null;
  jobFillEl.style.width = `${Math.round(frac * 100)}%`;
  jobMetaEl.textContent = `chunk ${p.done} of ${p.total} · ${elapsed.toFixed(0)}s elapsed` + (left != null ? ` · ~${left.toFixed(0)}s left` : "");
});
