// fileicons.ts — emoji glyphs per file type for the file tree.
//
// Emoji are chosen over an icon library to keep the frontend dependency-free;
// they render in every webview the app targets.

const NAME_ICON: Record<string, string> = {
  dockerfile: '🐳',
  makefile: '🔧',
  'package.json': '📦',
  'cargo.toml': '📦',
  'readme.md': '📖',
  license: '📜',
};

const EXT_ICON: Record<string, string> = {
  js: '📙', jsx: '📙', mjs: '📙', cjs: '📙',
  ts: '📘', tsx: '📘', mts: '📘', cts: '📘',
  svelte: '🧡', vue: '💚',
  rs: '🦀', py: '🐍', go: '🐹', rb: '💎', php: '🐘',
  java: '☕', kt: '☕', kts: '☕', scala: '☕',
  sh: '🖥️', bash: '🖥️', zsh: '🖥️', fish: '🖥️',
  json: '🔧', jsonc: '🔧',
  toml: '⚙️', yaml: '⚙️', yml: '⚙️', ini: '⚙️', cfg: '⚙️', conf: '⚙️', env: '⚙️',
  md: '📝', mdx: '📝', markdown: '📝',
  html: '🌐', htm: '🌐', xml: '🌐',
  css: '🎨', scss: '🎨', sass: '🎨', less: '🎨',
  png: '🖼️', jpg: '🖼️', jpeg: '🖼️', gif: '🖼️', webp: '🖼️', svg: '🖼️', ico: '🖼️', bmp: '🖼️',
  sql: '🗄️',
  pdf: '📕',
  zip: '🗜️', tar: '🗜️', gz: '🗜️', tgz: '🗜️', '7z': '🗜️', rar: '🗜️',
  lock: '🔒',
  txt: '📄', log: '📄',
};

const DEFAULT_ICON = '📄';

/** Pick an icon for a file given its name or full path. */
export function fileIcon(name: string): string {
  const base = (name.split('/').pop() ?? name).toLowerCase();
  if (NAME_ICON[base]) return NAME_ICON[base];
  const dot = base.lastIndexOf('.');
  if (dot > 0) {
    const icon = EXT_ICON[base.slice(dot + 1)];
    if (icon) return icon;
  }
  return DEFAULT_ICON;
}

/** Folder icon — open or closed. */
export function folderIcon(open: boolean): string {
  return open ? '📂' : '📁';
}
