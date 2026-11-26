#!/usr/bin/env node
// drive.mjs — headless driver for the preview harness. Zero dependencies: Node's
// built-in WebSocket + Edge/Chrome's DevTools protocol.
//
//   node preview/drive.mjs library --shot out.png
//   node preview/drive.mjs library --eval "document.querySelector('.note-card').click()" --shot out.png
//   node preview/drive.mjs popup --eval-file steps.js --shot out.png --size 1450x1000
//
// Every --eval runs in order (awaited if it returns a promise), each followed by a
// short settle so async invokes and repaints land. Console errors + uncaught
// exceptions are printed to stderr and make the exit code non-zero, so an agent
// can use this as a smoke test, not just a camera. Requires a static server
// on :8765 (`python -m http.server 8765` in the repo root).
import { spawn } from "node:child_process";
import { writeFileSync, readFileSync, existsSync } from "node:fs";

const args = process.argv.slice(2);
const page = args[0] && !args[0].startsWith("--") ? args.shift() : "library";
const opt = { evals: [], shot: null, size: "1450x1000", settle: 350, url: null, keep: false };
for (let i = 0; i < args.length; i++) {
  const a = args[i];
  if (a === "--eval") opt.evals.push(args[++i]);
  else if (a === "--eval-file") opt.evals.push(readFileSync(args[++i], "utf8"));
  else if (a === "--shot") opt.shot = args[++i];
  else if (a === "--size") opt.size = args[++i];
  else if (a === "--settle") opt.settle = Number(args[++i]);
  else if (a === "--url") opt.url = args[++i];
  else if (a === "--keep") opt.keep = true;
}
const [w, h] = opt.size.split("x").map(Number);
const url = opt.url || `http://127.0.0.1:8765/preview/${page}.html`;

const candidates = [
  process.env.BROWSER_BIN,
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
].filter(Boolean);
const bin = candidates.find((p) => existsSync(p));
if (!bin) { console.error("no Chromium-based browser found; set BROWSER_BIN"); process.exit(2); }

const port = 9222 + Math.floor(Math.random() * 500);
const browser = spawn(bin, [
  "--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run",
  "--remote-debugging-port=" + port, `--window-size=${w},${h}`,
  "--user-data-dir=" + process.env.TEMP + "/samplify-drive-" + port,
  "about:blank",
], { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function wsUrl() {
  for (let i = 0; i < 50; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      const t = list.find((x) => x.type === "page");
      if (t) return t.webSocketDebuggerUrl;
    } catch {}
    await sleep(100);
  }
  throw new Error("browser did not expose a page target");
}

let seq = 0;
const pending = new Map();
const problems = [];
const ws = new WebSocket(await wsUrl());
await new Promise((r) => (ws.onopen = r));
ws.onmessage = (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
  if (m.method === "Runtime.consoleAPICalled" && (m.params.type === "error" || m.params.type === "warning")) {
    const text = m.params.args.map((a) => a.value ?? a.description ?? "").join(" ");
    if (m.params.type === "error") problems.push("console.error: " + text);
    else console.error("console.warn: " + text);
  }
  if (m.method === "Runtime.exceptionThrown") {
    const d = m.params.exceptionDetails;
    problems.push("exception: " + (d.exception?.description || d.text));
  }
  if (m.method === "Log.entryAdded" && m.params.entry.level === "error" && !/favicon.ico/.test(m.params.entry.url || "")) {
    problems.push("log: " + m.params.entry.text + " " + (m.params.entry.url || ""));
  }
};
const send = (method, params = {}) => new Promise((resolve) => {
  const id = ++seq;
  pending.set(id, resolve);
  ws.send(JSON.stringify({ id, method, params }));
});

await send("Runtime.enable");
await send("Log.enable");
await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url });
await sleep(1200); // loader fetches library.html, injects styles + modules, first invokes settle
for (const code of opt.evals) {
  const res = await send("Runtime.evaluate", { expression: `(async()=>{${code}})()`, awaitPromise: true, returnByValue: true });
  if (res.result?.exceptionDetails) problems.push("eval: " + (res.result.exceptionDetails.exception?.description || res.result.exceptionDetails.text));
  else if (res.result?.result?.value !== undefined) console.log(JSON.stringify(res.result.result.value));
  await sleep(opt.settle);
}
if (opt.shot) {
  const res = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(opt.shot, Buffer.from(res.result.data, "base64"));
  console.log("wrote " + opt.shot);
}
ws.close();
if (!opt.keep) browser.kill();
if (problems.length) {
  console.error("PROBLEMS:\n  " + problems.join("\n  "));
  process.exit(1);
}
