import { invoke, listen, getCurrentWindow, convertFileSrc } from "./bridge.js";
import { TAGS } from "./tags.js";
import { WaveformSelector } from "./waveform.js";
import { icon, mountIcons } from "./icons.js";

// ---- DOM ----
const artEl = document.getElementById("art");
const titleEl = document.getElementById("title");
const artistEl = document.getElementById("artist");
const timeEl = document.getElementById("time");
const closeBtn = document.getElementById("close-btn");
const waveCanvas = document.getElementById("waveform-canvas");
const rulerCanvas = document.getElementById("ruler-canvas");
const inTimeEl = document.getElementById("in-time");
const outTimeEl = document.getElementById("out-time");
const selDurEl = document.getElementById("sel-dur");
const loopBtn = document.getElementById("loop-btn");
const zoomSelBtn = document.getElementById("zoom-sel-btn");
const fitBtn = document.getElementById("fit-btn");
const auditionBtn = document.getElementById("audition-btn");
const noteEl = document.getElementById("note");
const saveBtn = document.getElementById("save-btn");
const tagsEl = document.getElementById("tags");
const audio = document.getElementById("audition-audio");
const tempoSliderEl = document.getElementById("tempo-slider");
const tempoValEl = document.getElementById("tempo-val");

// ---- State ----
let meta = null;          // popup_ready payload
let durMs = 0;            // snapshot duration (milliseconds)
let playheadMs = 0;       // audition position, in milliseconds
let playing = false;
let looping = true;
let activeEdge = "out";   // which edge arrows nudge
let closing = false;
const selectedTags = new Set();

// ---- Tag chips ----
TAGS.forEach((t, i) => {
  const el = document.createElement("div");
  el.className = "chip";
  el.style.color = t.color;
  el.dataset.name = t.name;
  el.innerHTML = `<span class="num">${i + 1}</span> ${t.name}`;
  el.onclick = () => toggleTag(t.name, el);
  tagsEl.appendChild(el);
});
function toggleTag(name, el) {
  if (selectedTags.has(name)) { selectedTags.delete(name); el.classList.remove("on"); }
  else { selectedTags.add(name); el.classList.add("on"); }
}

// ---- Time helpers ----
function pad2(n) { return String(n).padStart(2, "0"); }
function fmtClock(ms) { if (ms < 0) ms = 0; const s = Math.floor(ms / 1000); return `${Math.floor(s / 60)}:${pad2(s % 60)}`; }
function trackMsAt(snapMs) { return meta.trigger_track_ms - (durMs - snapMs); }
function snapSecAtTrackMs(ms) { return clamp(durMs - (meta.trigger_track_ms - ms), 0, durMs) / 1000; }
function fmtTrackTenths(snapMs) {
  let ms = Math.max(0, trackMsAt(snapMs));
  const s = ms / 1000;
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  const t = Math.floor((s * 10) % 10);
  return `${m}:${pad2(sec)}.${t}`;
}
function parseTrackTenths(str) {
  const m = str.trim().match(/^(\d+):(\d{1,2})(?:\.(\d))?$/);
  if (!m) return null;
  const ms = (Number(m[1]) * 60 + Number(m[2])) * 1000 + (m[3] ? Number(m[3]) * 100 : 0);
  return ms;
}
function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }

// ---- Shared Waveform Selector ----
const waveform = new WaveformSelector(waveCanvas, waveCanvas.parentElement, {
  getPeaks: () => (meta ? meta.waveform : []),
  getDurationMs: () => durMs,
  getPlayheadMs: () => playheadMs,
  isPlaying: () => playing,
  isLooping: () => looping,
  minSelMs: 1000, // 1.0s MIN_SEL
  allowMiddleDrag: true,
  allowPan: true,
  renderStyle: 'card',
  onSelectionDragMs: (startMs, endMs, dragMode) => {
    updateReadout();
    if (playing && (dragMode === "left" || dragMode === "draw")) {
      audio.currentTime = startMs / 1000;
      playheadMs = startMs;
    }
  },
  onSelectionChange: (startMs, endMs) => {
    updateReadout();
  },
  onSeek: (ms) => {
    playheadMs = ms;
    audio.currentTime = ms / 1000;
    scheduleDraw();
  },
  onActiveEdgeChange: (edge) => {
    activeEdge = edge;
  }
});

// ---- Rendering (coalesced via rAF) ----
let drawQueued = false;
function scheduleDraw() {
  if (drawQueued) return;
  drawQueued = true;
  const run = () => { drawQueued = false; draw(); };
  if (document.hidden) setTimeout(run, 0);
  else requestAnimationFrame(run);
}

function draw() {
  if (!meta) return;
  waveform.draw();
  drawRuler();
  updateReadout();
}

function niceTick(spanSec) {
  const targets = [0.5, 1, 2, 5, 10, 15, 30, 60, 120];
  const want = spanSec / 6;
  for (const t of targets) if (t >= want) return t;
  return 300;
}

function drawRuler() {
  const dpr = window.devicePixelRatio || 1;
  const w = rulerCanvas.clientWidth;
  const h = rulerCanvas.clientHeight;
  rulerCanvas.width = Math.max(1, Math.round(w * dpr));
  rulerCanvas.height = Math.max(1, Math.round(h * dpr));
  const ctx = rulerCanvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  ctx.fillStyle = "rgba(138,138,144,0.9)";
  ctx.font = "9px system-ui, sans-serif";
  ctx.textBaseline = "middle";

  const viewStartMs = waveform.viewStartMs;
  const viewEndMs = waveform.viewEndMs;
  const viewDurMs = viewEndMs - viewStartMs;

  const step = niceTick(viewDurMs / 1000);
  const startTrackS = trackMsAt(viewStartMs) / 1000;
  let tS = Math.ceil(startTrackS / step) * step;
  const endTrackS = trackMsAt(viewEndMs) / 1000;

  for (; tS <= endTrackS; tS += step) {
    const snapMs = snapSecAtTrackMs(tS * 1000) * 1000;
    const x = waveform.msToX(snapMs, w);
    if (x < 0 || x > w) continue;
    ctx.fillStyle = "rgba(138,138,144,0.35)";
    ctx.fillRect(x, 0, 1, 5);
    ctx.fillStyle = "rgba(138,138,144,0.9)";
    ctx.fillText(fmtClock(tS * 1000), Math.min(x + 3, w - 26), h / 2 + 1);
  }
}

function updateReadout() {
  const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
  if (document.activeElement !== inTimeEl) inTimeEl.value = fmtTrackTenths(sel.startMs);
  if (document.activeElement !== outTimeEl) outTimeEl.value = fmtTrackTenths(sel.endMs);
  selDurEl.textContent = `${((sel.endMs - sel.startMs) / 1000).toFixed(1)}s`;
}

// ---- Selection mutations ----
function nudgeEdge(edge, deltaMs) {
  waveform.nudgeSelection(edge, deltaMs);
  activeEdge = edge;
}

fitBtn.onclick = () => { waveform.fit(); draw(); };
zoomSelBtn.onclick = () => { waveform.zoomToSelection(); draw(); };

// ---- Editable time inputs (track-time) ----
function commitTimeInput(el, which) {
  const ms = parseTrackTenths(el.value);
  if (ms == null) { updateReadout(); return; }
  const snapMs = snapSecAtTrackMs(ms) * 1000;
  const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
  if (which === "in") {
    waveform.setSelection(snapMs, sel.endMs);
  } else {
    waveform.setSelection(sel.startMs, snapMs);
  }
  updateReadout();
}
inTimeEl.addEventListener("keydown", (e) => { if (e.key === "Enter") { commitTimeInput(inTimeEl, "in"); inTimeEl.blur(); } e.stopPropagation(); });
outTimeEl.addEventListener("keydown", (e) => { if (e.key === "Enter") { commitTimeInput(outTimeEl, "out"); outTimeEl.blur(); } e.stopPropagation(); });
inTimeEl.addEventListener("blur", () => commitTimeInput(inTimeEl, "in"));
outTimeEl.addEventListener("blur", () => commitTimeInput(outTimeEl, "out"));

// ---- Audition ----
function startAudition() {
  const begin = () => {
    const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
    audio.currentTime = clamp(playheadMs >= sel.startMs && playheadMs < sel.endMs ? playheadMs : sel.startMs, sel.startMs, sel.endMs) / 1000;
    audio.play().then(() => {
      playing = true;
      auditionBtn.innerHTML = `${icon("pause", 13)} Pause`;
      auditionBtn.classList.add("playing");
      tickPlayhead();
    }).catch((err) => console.error("audition failed:", err));
  };
  if (audio.readyState >= 2) begin();
  else { audio.addEventListener("canplay", begin, { once: true }); audio.load(); }
}
function stopAudition() {
  audio.pause(); playing = false;
  auditionBtn.innerHTML = `${icon("play", 13)} Play`;
  auditionBtn.classList.remove("playing");
  scheduleDraw();
}

// Audition speed only — the saved WAV is always the raw selection, so a clip
// trimmed at 0.75x still writes exactly what the In/Out markers describe.
function applyTempo() {
  const rate = Number(tempoSliderEl.value) / 100;
  audio.playbackRate = rate;
  audio.preservesPitch = true;
  audio.mozPreservesPitch = true;
  audio.webkitPreservesPitch = true;
  tempoValEl.textContent = rate.toFixed(2) + "×";
}
tempoSliderEl.oninput = applyTempo;
tempoSliderEl.ondblclick = () => { tempoSliderEl.value = 100; applyTempo(); };
// Re-apply on each play — loading a new snapshot resets the rate.
audio.addEventListener("play", applyTempo);

function tickPlayhead() {
  if (!playing) return;
  playheadMs = audio.currentTime * 1000;
  const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
  if (playheadMs >= sel.endMs || playheadMs < sel.startMs) {
    if (looping) { audio.currentTime = sel.startMs / 1000; playheadMs = sel.startMs; }
    else { stopAudition(); playheadMs = sel.startMs; return; }
  }
  scheduleDraw();
  requestAnimationFrame(tickPlayhead);
}
auditionBtn.onclick = () => { if (!meta) return; playing ? stopAudition() : startAudition(); };
loopBtn.onclick = () => { looping = !looping; loopBtn.classList.toggle("active", looping); };

audio.addEventListener("ended", () => {
  const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
  if (looping && playing) { audio.currentTime = sel.startMs / 1000; audio.play().catch(() => {}); }
  else if (playing) { stopAudition(); playheadMs = sel.startMs; scheduleDraw(); }
});

// ---- Keyboard ----
function isTyping() { const a = document.activeElement; return a === noteEl || a === inTimeEl || a === outTimeEl; }
document.addEventListener("keydown", async (e) => {
  if (e.key === "Escape") { e.preventDefault(); return discard(); }
  if (e.key === "Enter" && !(document.activeElement === inTimeEl || document.activeElement === outTimeEl)) { e.preventDefault(); return save(); }
  if (isTyping()) return;
  if (e.key >= "1" && e.key <= "7") {
    e.preventDefault();
    const el = tagsEl.children[Number(e.key) - 1];
    if (el) toggleTag(el.dataset.name, el);
  } else if (e.key === " ") { e.preventDefault(); playing ? stopAudition() : startAudition(); }
  else if (e.key.toLowerCase() === "l") { loopBtn.click(); }
  else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
    e.preventDefault();
    const stepMs = e.shiftKey ? 2000 : 500;
    const dir = e.key === "ArrowRight" ? 1 : -1;
    nudgeEdge(e.altKey ? "in" : "out", dir * stepMs);
  } else if (e.key === "[") {
    const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
    waveform.setSelection(playheadMs, sel.endMs);
  } else if (e.key === "]") {
    const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
    waveform.setSelection(sel.startMs, playheadMs);
  }
});

// ---- Save / discard ----
function releaseAudio() { audio.pause(); audio.removeAttribute("src"); audio.load(); }
async function discard() {
  if (closing) return; closing = true;
  releaseAudio();
  try { await invoke("discard_capture_cmd"); } catch {}
  await getCurrentWindow().hide(); closing = false;
}
async function save() {
  if (!meta || closing) return; closing = true;
  const note = noteEl.value.trim();
  const tags = [...selectedTags];
  releaseAudio();
  const sel = waveform.getSelection() || { startMs: 0, endMs: durMs };
  try { await invoke("save_capture_cmd", { note, tags, startMs: sel.startMs, endMs: sel.endMs }); }
  catch (e) { console.error("save failed:", e); }
  await getCurrentWindow().hide(); closing = false;
}
saveBtn.onclick = save;
closeBtn.onclick = discard;

// ---- Init ----
async function initPopup() {
  try {
    meta = await invoke("popup_ready");
    durMs = meta.snapshot_duration_ms;
    artEl.onerror = () => { artEl.style.display = "none"; };
    artEl.src = meta.album_art_url || "";
    artEl.style.display = meta.album_art_url ? "block" : "none";
    titleEl.textContent = meta.title || "Unknown";
    artistEl.textContent = meta.artist || "";
    timeEl.textContent = fmtClock(meta.trigger_track_ms);
    // Every snapshot rewrites the same WAV. Give WebView's media cache a new
    // URL so the audition and waveform refer to the same take.
    const auditionUrl = new URL(convertFileSrc(meta.temp_wav_path));
    if (auditionUrl.host) auditionUrl.searchParams.set("capture", String(Date.now()));
    audio.src = auditionUrl.href;
    audio.load();

    const wantMs = Math.min(20000, durMs);
    const startMs = Math.max(0, durMs - wantMs);
    const endMs = durMs;

    waveform.setView(0, durMs);
    waveform.setSelection(startMs, endMs);

    playheadMs = startMs;
    playing = false; auditionBtn.innerHTML = `${icon("play", 13)} Play`; auditionBtn.classList.remove("playing");
    tempoSliderEl.value = 100; applyTempo();
    looping = true; loopBtn.classList.add("active");
    selectedTags.clear(); [...tagsEl.children].forEach((c) => c.classList.remove("on"));
    noteEl.value = ""; noteEl.focus();
    mountIcons();
    draw();
  } catch (e) { console.error("popup init failed", e); }
}

new ResizeObserver(() => draw()).observe(waveCanvas);
listen("popup-data", () => initPopup());
initPopup();
