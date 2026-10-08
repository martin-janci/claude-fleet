import { describe, it, expect } from 'vitest';
import { RAIL_ITEMS, currentRailItem, visibleRailItems } from './rail';
import { shortcutById } from './shortcuts';

describe('rail', () => {
  it('keeps the design manual order, Settings last', () => {
    expect(RAIL_ITEMS.map((i) => i.id)).toEqual([
      'control',
      'inbox',
      'sessions',
      'work',
      'automation',
      'accounts',
      'toolkit',
      'settings',
    ]);
  });

  it('hides every item whose step has not landed', () => {
    expect(visibleRailItems().map((i) => i.id)).toEqual(['inbox', 'sessions', 'work', 'accounts', 'settings']);
  });

  it('names only shortcuts the registry has', () => {
    for (const i of RAIL_ITEMS) if (i.shortcut) expect(shortcutById(i.shortcut), i.id).toBeTruthy();
  });

  it('Accounts covers Accounts and Hosts; a session overlay follows the sidebar tree', () => {
    expect(currentRailItem('accounts', 'sessions')).toBe('accounts');
    expect(currentRailItem('hosts', 'work')).toBe('accounts');
    expect(currentRailItem('session', 'sessions')).toBe('sessions');
    expect(currentRailItem('files', 'sessions')).toBe('sessions');
    expect(currentRailItem('board', 'work')).toBe('work');
    expect(currentRailItem('assets', 'sessions')).toBeNull();
    expect(currentRailItem('session', 'inbox')).toBe('inbox');
    expect(currentRailItem('files', 'inbox')).toBe('inbox');
  });
});
