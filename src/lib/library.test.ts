import { describe, it, expect } from 'vitest';
import { libraryEntries, matchesFilter, placedFiles, reposOf, type LibraryItem } from './library';
import type { Download } from './downloads';
import { session } from './hosts_fixture';

// Redesign step 9.7: Control's Library.

const dl = (over: Partial<Download>): Download => ({
  id: 1,
  at: 100,
  host_alias: 'mac',
  session_id: 4,
  session_name: 'fix-login',
  path: '/w/out/report.pdf',
  name: 'report.pdf',
  size: 2048,
  state: 'ready',
  source: 'agent',
  ...over,
});

const item: LibraryItem = {
  id: 9,
  at: 150,
  kind: 'upload',
  host_alias: 'trn',
  session_id: 5,
  session_name: 'release',
  path: '/w/.claude-fleet-attachments/spec.pdf',
  name: 'spec.pdf',
  size: 10,
};

const byId = new Map([
  [1, { project: { owner: 'acme', repo: 'web', system: false } }],
  [2, { project: { owner: 'fleet', repo: 'operator', system: true } }],
]);

describe('library', () => {
  it('lists a download from a session as that session’s output (the plan’s acceptance)', () => {
    const [e] = libraryEntries([], [dl({})], []);
    expect(e).toMatchObject({ kind: 'output', name: 'report.pdf', host: 'mac', session: 'fix-login', size: 2048 });
    expect(libraryEntries([], [dl({ source: 'person' })], [])[0].kind).toBe('download');
  });

  it('puts files newest first and repos after them, and leaves failed copies out', () => {
    const repos = reposOf([session('mac', 'a', { project_id: 1, created_at: 500 })], byId);
    const list = libraryEntries([item], [dl({}), dl({ id: 2, state: 'failed' })], repos);
    expect(list.map((e) => e.key)).toEqual(['lib:9', 'dl:1', 'repo:mac:1']);
  });

  it('a repo is listed once per host, and never fleet’s own working directory', () => {
    const rows = [
      session('mac', 'a', { project_id: 1, created_at: 10 }),
      session('mac', 'b', { project_id: 1, created_at: 30 }),
      session('trn', 'c', { project_id: 1, created_at: 20 }),
      session('mac', 'd', { project_id: 2 }),
      session('mac', 'e', { project_id: null }),
    ];
    const repos = reposOf(rows, byId);
    expect(repos.map((r) => [r.host, r.detail, r.at])).toEqual([
      ['mac', 'acme/web', 30],
      ['trn', 'acme/web', 20],
    ]);
  });

  it('filters by what an entry is', () => {
    const [output] = libraryEntries([], [dl({})], []);
    const [upload] = libraryEntries([item], [], []);
    expect(matchesFilter(output, 'files')).toBe(true);
    expect(matchesFilter(output, 'uploads')).toBe(false);
    expect(matchesFilter(upload, 'uploads')).toBe(true);
    expect(matchesFilter(upload, 'all')).toBe(true);
    expect(matchesFilter(upload, 'repos')).toBe(false);
  });

  it('pairs each host path with the file that was picked', () => {
    expect(
      placedFiles(
        [{ path: '/Users/m/a.png', name: 'a.png', size: 3, kind: 'image' }],
        ['/w/.claude-fleet-attachments/a.png'],
      ),
    ).toEqual([{ path: '/w/.claude-fleet-attachments/a.png', name: 'a.png', size: 3 }]);
  });
});
