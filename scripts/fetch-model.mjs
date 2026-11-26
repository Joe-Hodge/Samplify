// Fetches the htdemucs ONNX export into src-tauri/resources/models/ so the Tauri
// bundler can ship it. NOT committed: at 166 MB the file exceeds GitHub's 100 MB
// hard limit, so the repo stays clean and the installer carries the model.
// Hash is pinned — a substituted or truncated download must fail loudly.
import { createHash } from "node:crypto";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";

const NAME = "htdemucs_fp16weights.onnx";
const URL = `https://huggingface.co/StemSplitio/htdemucs-onnx/resolve/main/${NAME}`;
const SHA256 = "d05c269d0178d2a72ad484b10b11dd370193fc923201c3b27a99f848745db70a";
const BYTES = 165_612_636;

const dir = path.join("src-tauri", "resources", "models");
const dest = path.join(dir, NAME);

const sha = async (p) => createHash("sha256").update(await readFile(p)).digest("hex");

if (existsSync(dest)) {
  const got = await sha(dest);
  if (got === SHA256) {
    console.log(`[fetch-model] ${NAME} present and verified.`);
    process.exit(0);
  }
  console.warn(`[fetch-model] hash mismatch (${got}), refetching.`);
}

await mkdir(dir, { recursive: true });
console.log(`[fetch-model] downloading ${NAME} (~166 MB)…`);
const res = await fetch(URL);
if (!res.ok) throw new Error(`download failed: ${res.status} ${res.statusText}`);
const part = `${dest}.part`;
await writeFile(part, Buffer.from(await res.arrayBuffer()));

const got = await sha(part);
if (got !== SHA256) throw new Error(`SHA-256 mismatch: expected ${SHA256}, got ${got}`);
const size = (await readFile(part)).length;
if (size !== BYTES) throw new Error(`size mismatch: expected ${BYTES}, got ${size}`);
await rename(part, dest);
console.log(`[fetch-model] verified and installed.`);
