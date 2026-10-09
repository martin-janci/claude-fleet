// The manual's icon set (design-system README, Iconography): 16-unit inline
// SVG strokes at 1.5 px in currentColor, drawn at 12 to 18 px. The Rail's
// icons are the reference set; search, filter and clock come from
// ListFilters and AppHeader; the action icons after them replaced emoji. Each entry is the SVG's inner shapes as data.

export type OfIconShape =
  /** `fill`: a solid area in currentColor (half a todo's circle), drawn
   *  without a stroke of its own. */
  | { kind: 'path'; d: string; fill?: true }
  | { kind: 'circle'; cx: number; cy: number; r: number }
  | { kind: 'rect'; x: number; y: number; width: number; height: number; rx: number };

const p = (d: string): OfIconShape => ({ kind: 'path', d });
const solid = (d: string): OfIconShape => ({ kind: 'path', d, fill: true });
const c = (cx: number, cy: number, r: number): OfIconShape => ({ kind: 'circle', cx, cy, r });
const r = (x: number, y: number, width: number, height: number, rx: number): OfIconShape => ({
  kind: 'rect',
  x,
  y,
  width,
  height,
  rx,
});

export const OF_ICONS = {
  control: [c(8, 8, 5.5), c(8, 8, 2), p('M8 1v2.5M8 12.5V15M1 8h2.5M12.5 8H15')],
  inbox: [p('M2 9.5h3.5l1 1.5h3l1-1.5H14'), p('M3.5 3h9L14 9.5V13H2V9.5z')],
  sessions: [r(2, 3, 12, 10, 1.5), p('M4.5 6.5 6.5 8l-2 1.5M8 10h3')],
  work: [r(2.5, 2.5, 11, 11, 2), p('m5.5 8 1.8 1.8L10.8 6')],
  automation: [p('M13 8a5 5 0 1 1-1.5-3.6'), p('M13 2.5v2.5h-2.5'), p('M8 5.5V8l1.8 1.2')],
  accounts: [c(8, 5.5, 2.5), p('M3 13.5c.6-2.6 2.6-4 5-4s4.4 1.4 5 4')],
  toolkit: [p('M9.5 2.5a3 3 0 0 0-3 4L2.5 10.5l3 3 4-4a3 3 0 0 0 4-3l-2 1-2-2z')],
  settings: [c(8, 8, 2), p('M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M3.6 12.4 5 11M11 5l1.4-1.4')],
  search: [c(7, 7, 4.5), p('m10.5 10.5 3 3')],
  filter: [p('M2 3.5h12L9.5 9v4l-3-1.5V9z')],
  clock: [c(8, 8, 6), p('M8 4.5V8l2.5 1.5')],
  // Row and toolbar actions that used to be emoji (review r14 backlog).
  mic: [r(6, 2, 4, 7.5, 2), p('M3.8 7.5a4.2 4.2 0 0 0 8.4 0M8 11.7V14M5.5 14h5')],
  tag: [p('M2.5 2.5h5.2l5.8 5.8-5.2 5.2-5.8-5.8z'), c(5.5, 5.5, 1)],
  edit: [p('M10.5 2.5l3 3L6 13H3v-3z'), p('M9 4l3 3')],
  recreate: [p('M2.5 7a5.5 5.5 0 0 1 10-2'), p('M12.5 2.5V5H10'), p('M13.5 9a5.5 5.5 0 0 1-10 2'), p('M3.5 13.5V11H6')],
  checklist: [p('m2.5 4.5 1.2 1.2L6 3.5'), p('M8 4.5h5.5'), p('m2.5 10.5 1.2 1.2L6 9.5'), p('M8 10.5h5.5')],
  warning: [p('M8 2.5 14 13H2z'), p('M8 6.5v3'), p('M8 11.2v.3')],
  dice: [r(2.5, 2.5, 11, 11, 2), c(5.5, 5.5, 0.4), c(10.5, 5.5, 0.4), c(8, 8, 0.4), c(5.5, 10.5, 0.4), c(10.5, 10.5, 0.4)],
  trash: [p('M2.5 4.5h11M6 4.5V3h4v1.5'), p('M4 4.5l.7 9h6.6l.7-9'), p('M6.8 7v4.5M9.2 7v4.5')],
  bolt: [p('M9 1.5 3.5 9H8l-1 5.5L12.5 7H8z')],
  // The Conversation view's line icons, redrawn from the retired 24-unit set
  // (lib/Icon.svelte) on this grid, and the Assets rail's and the switcher's.
  copy: [r(6, 6, 8, 8, 1.5), p('M10 3.5V3a1 1 0 0 0-1-1H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h.5')],
  check: [p('m3.5 8.5 3 3 6-6.5')],
  quote: [p('M3 3.5v9'), p('M6 5h7M6 8h7M6 11h4.5')],
  retry: [p('M13.5 8a5.5 5.5 0 1 1-1.6-3.9'), p('M13.5 2.5v3h-3')],
  fork: [c(4.5, 3.5, 1.5), c(4.5, 12.5, 1.5), c(11.5, 4.5, 1.5), p('M4.5 5v6'), p('M11.5 6c0 2.7-2.7 3.3-5.5 4.3')],
  rewind: [p('M2.5 8a5.5 5.5 0 1 0 1.6-3.9'), p('M2.5 2.5v3h3'), p('M8 5.5v3l2 1.3')],
  file: [p('M9.5 2H5a1.5 1.5 0 0 0-1.5 1.5v9A1.5 1.5 0 0 0 5 14h6a1.5 1.5 0 0 0 1.5-1.5V5z'), p('M9.5 2v3h3')],
  terminal: [p('m3.5 4.5 3.5 3.5-3.5 3.5'), p('M8.5 12h4')],
  list: [p('M6 4h7.5M6 8h7.5M6 12h7.5'), c(2.75, 4, 0.5), c(2.75, 8, 0.5), c(2.75, 12, 0.5)],
  circle: [c(8, 8, 5.5)],
  'circle-half': [c(8, 8, 5.5), solid('M8 2.5a5.5 5.5 0 0 1 0 11z')],
  'circle-check': [c(8, 8, 5.5), p('m5.6 8.2 1.7 1.7 3.1-3.3')],
  library: [r(2, 2, 5, 5, 1), r(9, 2, 5, 5, 1), r(2, 9, 5, 5, 1), r(9, 9, 5, 5, 1)],
  layers: [p('M8 2 14 5 8 8 2 5z'), p('M2 8l6 3 6-3'), p('M2 11l6 3 6-3')],
  hosts: [r(2, 2.5, 12, 4.5, 1), r(2, 9, 12, 4.5, 1), c(4.5, 4.75, 0.5), c(4.5, 11.25, 0.5)],
  key: [c(5.5, 10.5, 3), p('m7.6 8.4 6-6M11.5 4.5l2 2')],
  disk: [r(2, 4, 12, 8, 1.5), c(11, 8, 0.5), p('M4.5 8h3')],
  upgrade: [p('M8 13.5V3'), p('m3.5 7.5 4.5-4.5 4.5 4.5')],
  link: [p('M7 9a2.5 2.5 0 0 0 3.5 0l2.5-2.5a2.5 2.5 0 0 0-3.5-3.5L8.5 4'), p('M9 7a2.5 2.5 0 0 0-3.5 0L3 9.5A2.5 2.5 0 0 0 6.5 13l1-1')],
  agent: [r(3, 5, 10, 8, 2), p('M8 2.5V5'), c(6, 9, 0.6), c(10, 9, 0.6), p('M1.5 8.5v2M14.5 8.5v2')],
  play: [p('M5 3.5v9l7-4.5z')],
  lock: [r(3, 7, 10, 7, 1.5), p('M5.5 7V5a2.5 2.5 0 0 1 5 0v2')],
  pause: [p('M6 3.5v9M10 3.5v9')],
  pin: [p('M8 11v3.5'), p('M6 3h4M6.5 3v3.5L4.5 9v2h7V9l-2-2.5V3')],
  folder: [p('M2 4.5A1.5 1.5 0 0 1 3.5 3h2.8l1.4 1.5h4.8A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z')],
  hide: [p('M6.6 3.7A6.5 6.5 0 0 1 8 3.5c4.5 0 6.5 4.5 6.5 4.5a9 9 0 0 1-1.2 1.8'), p('M4.3 4.3A9 9 0 0 0 1.5 8s2 4.5 6.5 4.5a6 6 0 0 0 3.5-1.1'), p('m2 2 12 12'), p('M9.4 9.5a2 2 0 0 1-2.9-2.9')],
} satisfies Record<string, OfIconShape[]>;

export type OfIconName = keyof typeof OF_ICONS;
