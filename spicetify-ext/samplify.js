// Samplify — Spicetify extension.
// Bridges the running Samplify desktop app (ws://127.0.0.1:5724) into the Spotify
// client: pushes accurate playback position, draws your note markers on the
// seekbar, and adds a Capture button to the playbar.
//
// Install: `spicetify config extensions samplify.js && spicetify apply`
// (copy this file into your Spicetify `Extensions` folder first — see README).
//
// NOTE: The seekbar-marker DOM selectors target Spotify's progress bar, which
// Spotify occasionally refactors. If markers stop appearing after a Spotify
// update, only the SELECTORS in `findProgressBar()` need adjusting — the rest
// keeps working.

(function samplify() {
  const BRIDGE_URL = "ws://127.0.0.1:5724";
  const RECONNECT_MS = 3000;

  // Wait until Spicetify's APIs are ready. Do not require Playbar — Spotify 1.3
  // hashed away the class Playbar.Button mounts into, so we inject via testids.
  if (!window.Spicetify || !Spicetify.Player || !Spicetify.Platform) {
    setTimeout(samplify, 300);
    return;
  }

  let ws = null;
  let connected = false;
  let currentId = null;
  let currentDurationMs = 0;
  let marks = [];

  // ---- WebSocket to the Samplify app ----
  function connect() {
    try {
      ws = new WebSocket(BRIDGE_URL);
    } catch (e) {
      scheduleReconnect();
      return;
    }
    ws.onopen = () => {
      connected = true;
      pushPosition();
      requestMarks();
    };
    ws.onclose = () => { connected = false; scheduleReconnect(); };
    ws.onerror = () => { try { ws.close(); } catch (e) {} };
    ws.onmessage = (ev) => {
      let msg;
      try { msg = JSON.parse(ev.data); } catch (e) { return; }
      if (msg.type === "marks" && msg.spotify_id === currentId) {
        marks = msg.marks || [];
        renderMarkers();
      } else if (msg.type === "request_position") {
        pushPosition();
      } else if (msg.type === "capture_result") {
        try { Spicetify.showNotification(msg.error || "Captured in Samplify", !!msg.error); } catch (e) {}
      } else if (msg.type === "pause") {
        try { if (Spicetify.Player.isPlaying()) Spicetify.Player.pause(); } catch (e) {}
      } else if (msg.type === "resume") {
        try { if (!Spicetify.Player.isPlaying()) Spicetify.Player.play(); } catch (e) {}
      } else if (msg.type === "refresh_marks") {
        requestMarks();
      }
    };
  }
  let reconnectTimer = null;
  function scheduleReconnect() {
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(() => { reconnectTimer = null; connect(); }, RECONNECT_MS);
  }
  function send(obj) {
    if (connected && ws && ws.readyState === WebSocket.OPEN) {
      try { ws.send(JSON.stringify(obj)); } catch (e) {}
    }
  }

  // ---- Track / playback state ----
  function trackUri() {
    const d = Spicetify.Player.data;
    return (d && (d.item?.uri || d.track?.uri || d.item?.metadata?.uri)) || null;
  }
  function trackIdFromUri(uri) {
    // "spotify:track:XXXX" -> "XXXX"; ignore non-track items (podcasts/ads).
    if (!uri || !uri.startsWith("spotify:track:")) return null;
    return uri.split(":")[2];
  }
  // Defensive metadata extraction — Spotify's Player.data shape varies by version.
  function trackMeta() {
    const d = Spicetify.Player.data || {};
    const item = d.item || d.track || {};
    const meta = item.metadata || {};
    const title = item.name || meta.title || "";
    let artist = "";
    if (Array.isArray(item.artists) && item.artists.length) {
      artist = item.artists.map((a) => a && a.name).filter(Boolean).join(", ");
    } else {
      artist = meta.artist_name || "";
    }
    const album = (item.album && item.album.name) || meta.album_title || "";
    let art = null;
    if (item.album && Array.isArray(item.album.images) && item.album.images.length) {
      art = item.album.images[item.album.images.length - 1].url;
    } else {
      art = meta.image_xlarge_url || meta.image_large_url || meta.image_url || null;
    }
    // Convert `spotify:image:ID` URIs (which the webview can't load) to CDN URLs.
    if (art && art.indexOf("spotify:image:") === 0) {
      art = "https://i.scdn.co/image/" + art.slice("spotify:image:".length);
    }
    const dur =
      (item.duration && item.duration.milliseconds) ||
      item.duration_ms ||
      Number(meta.duration) ||
      Spicetify.Player.getDuration() ||
      0;
    return { title, artist, album, album_art_url: art, duration_ms: Math.round(dur) };
  }

  function pushPosition() {
    const id = trackIdFromUri(trackUri());
    if (!id) { send({ type: "position", spotify_id: "" }); return; }
    const m = trackMeta();
    send({
      type: "position",
      spotify_id: id,
      position_ms: Math.round(Spicetify.Player.getProgress() || 0),
      is_playing: !!Spicetify.Player.isPlaying(),
      title: m.title,
      artist: m.artist,
      album: m.album,
      album_art_url: m.album_art_url,
      duration_ms: m.duration_ms,
    });
  }
  function requestMarks() {
    const id = trackIdFromUri(trackUri());
    if (id) send({ type: "request_marks", spotify_id: id });
  }

  function onSongChange() {
    const id = trackIdFromUri(trackUri());
    currentId = id;
    currentDurationMs = Spicetify.Player.getDuration() || 0;
    marks = [];
    renderMarkers();
    pushPosition();
    requestMarks();
  }

  // ---- Seekbar markers ----
  const OVERLAY_ID = "samplify-marker-overlay";

  // Custom tooltip (the native title attr is tiny and slow to appear).
  let tipEl = null;
  function ensureTip() {
    if (tipEl && document.body.contains(tipEl)) return tipEl;
    tipEl = document.createElement("div");
    tipEl.id = "samplify-tip";
    tipEl.style.cssText =
      "position:fixed;z-index:99999;display:none;pointer-events:none;" +
      "background:#161619;color:#f5f0e8;border:1px solid #E8912D;border-radius:8px;" +
      "padding:7px 11px;font-size:12px;line-height:1.45;max-width:280px;" +
      "box-shadow:0 6px 18px rgba(0,0,0,.55);transform:translate(-50%,-100%);";
    document.body.appendChild(tipEl);
    return tipEl;
  }
  function fmtMs(ms) {
    const s = Math.floor(ms / 1000);
    return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
  }
  function showTip(dot, m) {
    const tip = ensureTip();
    tip.replaceChildren();
    function line(text, style) {
      const el = document.createElement("div");
      el.textContent = text;
      el.style.cssText = style;
      tip.appendChild(el);
    }
    line(fmtMs(m.track_ms), "color:#E8912D;font-weight:700");
    line(m.note_text || "(no note)", "");
    if (m.tags && m.tags.length) line(m.tags.join(" · "), "color:#E8912D;margin-top:2px");
    line("click to jump · captured in Samplify", "color:#888;margin-top:3px;font-size:10px");
    const r = dot.getBoundingClientRect();
    tip.style.left = (r.left + r.width / 2) + "px";
    tip.style.top = (r.top - 10) + "px";
    tip.style.display = "block";
  }
  function hideTip() { if (tipEl) tipEl.style.display = "none"; }

  function findProgressBar() {
    // The seekable progress bar spans exactly the draggable region (time labels
    // are siblings, not inside it), so positioning markers by % of it aligns.
    const selectors = [
      '[data-testid="playback-progressbar"]',
      '[data-testid="progress-bar"]',
      '[data-testid="now-playing-bar"] [role="slider"]',
      ".playback-progressbar",
      ".playback-bar > div:nth-child(2)",
      ".playback-bar",
      ".x-progressBar-progressBarBg",
    ];
    for (const s of selectors) {
      const el = document.querySelector(s);
      if (el) return el;
    }
    return null;
  }

  function renderMarkers() {
    hideTip(); // a hovered marker may have just been deleted/moved
    const bar = findProgressBar();
    if (!bar) {
      console.warn("[Samplify] progress bar not found — markers can't render (DOM selector needs updating)");
      return;
    }
    if (getComputedStyle(bar).position === "static") bar.style.position = "relative";
    bar.style.overflow = "visible"; // let markers stand above the thin bar

    let overlay = document.getElementById(OVERLAY_ID);
    if (!overlay) {
      overlay = document.createElement("div");
      overlay.id = OVERLAY_ID;
      overlay.style.cssText = "position:absolute;inset:0;overflow:visible;pointer-events:none;z-index:100;";
      bar.appendChild(overlay);
    }
    overlay.innerHTML = "";
    const dur = currentDurationMs || Spicetify.Player.getDuration() || 0;
    if (!dur) return;
    console.log(`[Samplify] rendering ${marks.length} marker(s) on`, bar);

    for (const m of marks) {
      const pct = Math.max(0, Math.min(100, (m.track_ms / dur) * 100));
      const dot = document.createElement("div");
      dot.style.cssText =
        "position:absolute;top:50%;transform:translate(-50%,-50%) rotate(45deg);" +
        "width:10px;height:10px;background:#E8912D;border:1px solid rgba(0,0,0,.5);" +
        "box-shadow:0 0 3px rgba(0,0,0,.6);pointer-events:auto;cursor:pointer;left:" + pct + "%;";
      dot.addEventListener("mouseenter", () => showTip(dot, m));
      dot.addEventListener("mouseleave", hideTip);
      dot.addEventListener("click", (e) => {
        e.stopPropagation();
        hideTip();
        try { Spicetify.Player.seek(m.track_ms); } catch (err) {}
      });
      overlay.appendChild(dot);
    }
  }

  // Re-inject markers if Spotify re-renders the playback bar.
  const observer = new MutationObserver(() => {
    if (marks.length && !document.getElementById(OVERLAY_ID)) renderMarkers();
  });
  observer.observe(document.body, { childList: true, subtree: true });

  // ---- Capture button ----
  // Spotify 1.3 hashed `.main-nowPlayingBar-right`, so Spicetify.Playbar.Button
  // never mounts. Inject onto stable data-testids instead.
  const BTN_ID = "samplify-capture-btn";
  const CAPTURE_ICON =
    '<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">' +
    '<rect x="1" y="6" width="1.5" height="4"/><rect x="4" y="3" width="1.5" height="10"/>' +
    '<rect x="7" y="1" width="1.5" height="14"/><rect x="10" y="4" width="1.5" height="8"/>' +
    '<rect x="13.5" y="6" width="1.5" height="4"/></svg>';

  function onCaptureClick() {
    if (!connected) {
      try { Spicetify.showNotification("Samplify app not connected — is it running?"); } catch (e) {}
      return;
    }
    pushPosition();
    send({ type: "capture" });
  }

  function findButtonHost() {
    return (
      document.querySelector('[data-testid="general-controls"]') ||
      document.querySelector(".main-nowPlayingBar-right") ||
      document.querySelector('[data-testid="player-controls"]') ||
      document.querySelector('[data-testid="now-playing-bar"]')
    );
  }

  function mountCaptureButton() {
    if (document.getElementById(BTN_ID)) return true;
    const host = findButtonHost();
    if (!host) return false;
    const btn = document.createElement("button");
    btn.id = BTN_ID;
    btn.type = "button";
    btn.setAttribute("aria-label", "Samplify capture");
    btn.title = "Samplify capture";
    btn.style.cssText =
      "display:inline-flex;align-items:center;justify-content:center;gap:6px;" +
      "height:32px;padding:0 10px;margin:0 8px 0 4px;border:none;border-radius:16px;" +
      "background:#E8912D;color:#140d04;font-size:12px;font-weight:700;cursor:pointer;" +
      "flex:0 0 auto;z-index:5;font-family:inherit;";
    btn.innerHTML = CAPTURE_ICON + "<span>Capture</span>";
    btn.addEventListener("click", (e) => { e.stopPropagation(); onCaptureClick(); });
    host.prepend(btn);
    console.log("[Samplify] capture button mounted on", host.getAttribute("data-testid") || host.className);
    return true;
  }

  function ensureCaptureButton() {
    if (!mountCaptureButton()) setTimeout(ensureCaptureButton, 400);
  }
  ensureCaptureButton();
  new MutationObserver(() => {
    if (!document.getElementById(BTN_ID)) mountCaptureButton();
  }).observe(document.body, { childList: true, subtree: true });

  try {
    if (Spicetify.Playbar && Spicetify.Playbar.Button) {
      new Spicetify.Playbar.Button(
        "Samplify capture",
        CAPTURE_ICON,
        onCaptureClick,
        false,
        false
      );
    }
  } catch (e) {
    console.warn("[Samplify] Playbar.Button skipped", e);
  }

  // ---- Wire up ----
  connect();
  Spicetify.Player.addEventListener("songchange", onSongChange);
  Spicetify.Player.addEventListener("onplaypause", pushPosition);
  setInterval(() => { if (connected) pushPosition(); }, 500);
  onSongChange();

  console.log("[Samplify] extension loaded");
})();
