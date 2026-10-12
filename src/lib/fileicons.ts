// The icon a file or folder row shows in the file browser, from the manual's
// line set (kit/icons.ts; design-system README, Iconography). The manual has
// no per-language marks, so a file's type reads from its name: the icon only
// separates what behaves differently (a folder, a script, a lock file, a
// config, a document).
import type { OfIconName } from './kit/icons';

const NAME_ICON: Record<string, OfIconName> = {
  dockerfile: 'settings',
  makefile: 'terminal',
  'package.json': 'settings',
  'cargo.toml': 'settings',
  'readme.md': 'list',
  license: 'list',
};

const EXT_ICON: Record<string, OfIconName> = {
  sh: 'terminal', bash: 'terminal', zsh: 'terminal', fish: 'terminal',
  json: 'settings', jsonc: 'settings',
  toml: 'settings', yaml: 'settings', yml: 'settings', ini: 'settings', cfg: 'settings', conf: 'settings', env: 'settings',
  md: 'list', mdx: 'list', markdown: 'list', txt: 'list', log: 'list',
  lock: 'lock',
};

const DEFAULT_ICON: OfIconName = 'file';

/** Pick an icon for a file given its name or full path. */
export function fileIcon(name: string): OfIconName {
  const base = (name.split('/').pop() ?? name).toLowerCase();
  if (NAME_ICON[base]) return NAME_ICON[base];
  const dot = base.lastIndexOf('.');
  if (dot > 0) {
    const icon = EXT_ICON[base.slice(dot + 1)];
    if (icon) return icon;
  }
  return DEFAULT_ICON;
}

/** Folder icon: the same mark open or closed (the row's chevron says which). */
export function folderIcon(_open: boolean): OfIconName {
  return 'folder';
}
