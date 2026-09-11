import { describe, it, expect } from 'vitest';
import { describePurge, purgeHostsForProject } from './purge';
import type { PurgeReport, SessionRow } from './sessions';

const row = (project_id: number | null, host_alias: string) =>
  ({ project_id, host_alias }) as SessionRow;

describe('purgeHostsForProject', () => {
  it("returns the hosts of the project's sessions, not 'local'", () => {
    const rows = [row(1, 'mefistos'), row(2, 'local'), row(1, 'mefistos'), row(1, 'turanga')];
    expect(purgeHostsForProject(1, rows)).toEqual(['mefistos', 'turanga']);
  });

  it("falls back to 'local' (where the scan found it) when the project has no sessions", () => {
    expect(purgeHostsForProject(3, [row(1, 'mefistos'), row(null, 'box')])).toEqual(['local']);
  });
});

describe('describePurge', () => {
  const base: PurgeReport = {
    host_alias: 'local',
    logical_path: '/home/u/p/x',
    physical_path: '/mnt/p/x',
    purged: [],
    not_found: [],
  };

  it('names purged and not-found forms per host', () => {
    const t = describePurge([{ ...base, purged: ['/mnt/p/x'], not_found: ['/home/u/p/x'] }]);
    expect(t.kind).toBe('success');
    expect(t.message).toBe(
      'Project removed. local: purged /mnt/p/x; no Claude state for /home/u/p/x',
    );
  });

  it('reports an unresolved physical path and nothing purged as info', () => {
    const t = describePurge([
      { ...base, host_alias: 'box', physical_path: null, not_found: ['/home/u/p/x'] },
    ]);
    expect(t.kind).toBe('info');
    expect(t.message).toContain('box: no Claude state for /home/u/p/x');
    expect(t.message).toContain('physical path not resolved');
  });
});
