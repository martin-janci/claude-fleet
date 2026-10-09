// The manual's icon set (design-system README, Iconography): 16-unit inline
// SVG strokes at 1.5 px in currentColor, drawn at 12 to 18 px. The Rail's
// icons are the reference set; search, filter and clock come from
// ListFilters and AppHeader; the action icons after them replaced emoji. Each entry is the SVG's inner shapes as data.

export type OfIconShape =
  | { kind: 'path'; d: string }
  | { kind: 'circle'; cx: number; cy: number; r: number }
  | { kind: 'rect'; x: number; y: number; width: number; height: number; rx: number };

const p = (d: string): OfIconShape => ({ kind: 'path', d });
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
} satisfies Record<string, OfIconShape[]>;

export type OfIconName = keyof typeof OF_ICONS;
