import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { destination, destinationFlag, goTo, leave } from './destination';
import { hostsViewOpen, workBoardOpen } from './app_views';

beforeEach(() => destination.set('session'));

describe('destination', () => {
  it('holds one destination, so opening one overlay leaves the other', () => {
    goTo('hosts');
    goTo('assets');
    expect(get(destination)).toBe('assets');
    expect(get(hostsViewOpen)).toBe(false);
  });

  it('leave() returns to the Session tab only from the destination it names', () => {
    goTo('files');
    leave('board');
    expect(get(destination)).toBe('files');
    leave('files');
    expect(get(destination)).toBe('session');
  });

  it('a flag opens, closes and toggles its destination', () => {
    const files = destinationFlag('files');
    files.set(true);
    expect(get(destination)).toBe('files');
    files.update((v) => !v);
    expect(get(destination)).toBe('session');
    files.update((v) => !v);
    expect(get(files)).toBe(true);
  });

  it('closing a flag that is not open leaves the current overlay alone', () => {
    goTo('hosts');
    workBoardOpen.set(false);
    expect(get(destination)).toBe('hosts');
  });

  it('the Board button toggle replaces whatever overlay was open', () => {
    goTo('assets');
    workBoardOpen.update((v) => !v);
    expect(get(destination)).toBe('board');
    workBoardOpen.update((v) => !v);
    expect(get(destination)).toBe('session');
  });
});
