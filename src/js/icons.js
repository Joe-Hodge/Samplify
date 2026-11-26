// icons.js — the app's icon set as inline SVG strings. Stroke icons on a 24px
// grid, coloured by `currentColor`, so they follow the text colour of whatever
// they sit in and never need a font or an emoji fallback.
const PATHS = {
  library:     '<path d="M4 6h16M4 12h16M4 18h10"/>',
  folder:      '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  tag:         '<path d="M20 12 12 20l-9-9V3h8z"/><circle cx="7.5" cy="7.5" r="1.2" fill="currentColor" stroke="none"/>',
  sliders:     '<path d="M4 6h10M18 6h2M4 12h3M11 12h9M4 18h12M20 18h0"/><circle cx="16" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="18" cy="18" r="2"/>',
  settings:    '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>',
  play:        '<path d="M7 4.5v15l12-7.5z" fill="currentColor" stroke="none"/>',
  pause:       '<rect x="6" y="4.5" width="4" height="15" rx="1" fill="currentColor" stroke="none"/><rect x="14" y="4.5" width="4" height="15" rx="1" fill="currentColor" stroke="none"/>',
  stop:        '<rect x="6" y="6" width="12" height="12" rx="1.5" fill="currentColor" stroke="none"/>',
  star:        '<path d="m12 3 2.8 5.9 6.4.8-4.7 4.4 1.2 6.4L12 17.4l-5.7 3.1 1.2-6.4L2.8 9.7l6.4-.8z"/>',
  trash:       '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/>',
  grip:        '<circle cx="9" cy="6" r="1.4" fill="currentColor" stroke="none"/><circle cx="15" cy="6" r="1.4" fill="currentColor" stroke="none"/><circle cx="9" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="15" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="9" cy="18" r="1.4" fill="currentColor" stroke="none"/><circle cx="15" cy="18" r="1.4" fill="currentColor" stroke="none"/>',
  more:        '<circle cx="12" cy="5" r="1.6" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none"/><circle cx="12" cy="19" r="1.6" fill="currentColor" stroke="none"/>',
  stems:       '<path d="M4 6h16M4 10h16M4 14h16M4 18h16"/><path d="M8 4v4M14 8v4M10 12v4M16 16v4" stroke-width="2.6"/>',
  grid:        '<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>',
  export:      '<path d="M7 17 17 7M9 7h8v8"/>',
  download:    '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>',
  volume:      '<path d="M11 5 6 9H3v6h3l5 4z" fill="currentColor" stroke="none"/><path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13"/>',
  speed:       '<path d="M4 6v12l8-6zM12 6v12l8-6z" fill="currentColor" stroke="none"/>',
  loop:        '<path d="M17 2l4 4-4 4"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><path d="M7 22l-4-4 4-4"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/>',
  close:       '<path d="M6 6l12 12M18 6 6 18"/>',
  check:       '<path d="m5 12 5 5L20 7"/>',
  search:      '<circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.3-4.3"/>',
  music:       '<path d="M9 18V6l11-2v12"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/>',
  zoomIn:      '<path d="M12 5v14M5 12h14"/>',
  zoomOut:     '<path d="M5 12h14"/>',
  fit:         '<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>',
  capture:     '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="3" fill="currentColor" stroke="none"/>',
  mic:         '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>',
  drum:        '<ellipse cx="12" cy="8" rx="8" ry="3"/><path d="M4 8v8c0 1.7 3.6 3 8 3s8-1.3 8-3V8M20 3l-7 5.5M4 3l7 5.5"/>',
  bass:        '<path d="M6 20a4 4 0 0 1 0-8c1 0 2 .5 2.5 1L18 4l2 2-9 9.5c.5.5 1 1.5 1 2.5a4 4 0 0 1-6 3.5"/>',
  wave:        '<path d="M3 12h2l2-6 3 12 3-9 2 5 2-2h4"/>',
  back:        '<path d="M15 6l-6 6 6 6"/>',
  info:        '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h0"/>',
  warning:     '<path d="M12 3 2 20h20zM12 10v4M12 17h0"/>',
};

/**
 * @param {keyof typeof PATHS} name
 * @param {number} size  rendered size in CSS px
 * @param {string} cls   extra class names
 */
export function icon(name, size = 16, cls = "") {
  const d = PATHS[name];
  if (!d) return "";
  return `<svg class="ico ${cls}" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${d}</svg>`;
}

// Swap every `<i data-icon="name" data-size="16">` placeholder in static HTML
// for its SVG, so markup can name an icon without inlining paths.
export function mountIcons(root = document) {
  root.querySelectorAll("i[data-icon]").forEach((el) => {
    const svg = icon(el.dataset.icon, Number(el.dataset.size || 16), el.className);
    if (svg) el.outerHTML = svg;
  });
}
