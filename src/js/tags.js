// tags.js — the LOCKED tag set (order = number hotkey 1..7)
export const TAGS = [
  { name: "break",   color: "var(--tag-break)" },
  { name: "vocal",   color: "var(--tag-vocal)" },
  { name: "melody",  color: "var(--tag-melody)" },
  { name: "bass",    color: "var(--tag-bass)" },
  { name: "chords",  color: "var(--tag-chords)" },
  { name: "texture", color: "var(--tag-texture)" },
  { name: "fx",      color: "var(--tag-fx)" },
];
export function fmtTime(ms) {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
