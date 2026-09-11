import type { PurgeReport, SessionRow } from './sessions';

/**
 * Hosts a project purge must run on. Projects carry no host of their own:
 * the project scan is local, while its sessions (and so Claude's transcripts)
 * can live on any host. Purge wherever the project's sessions ran, falling
 * back to `local` (where the scan found the project) when it has none.
 */
export function purgeHostsForProject(projectId: number, rows: readonly SessionRow[]): string[] {
  const hosts: string[] = [];
  for (const s of rows) {
    if (s.project_id === projectId && !hosts.includes(s.host_alias)) hosts.push(s.host_alias);
  }
  return hosts.length > 0 ? hosts : ['local'];
}

/** Toast describing what the purge did on each host. */
export function describePurge(reports: readonly PurgeReport[]): {
  kind: 'success' | 'info';
  message: string;
} {
  const perHost = reports.map((r) => {
    const bits: string[] = [];
    if (r.purged.length > 0) bits.push(`purged ${r.purged.join(' and ')}`);
    if (r.not_found.length > 0) bits.push(`no Claude state for ${r.not_found.join(' and ')}`);
    if (r.physical_path === null) bits.push('directory missing, physical path not resolved');
    return `${r.host_alias}: ${bits.join('; ')}`;
  });
  const anyPurged = reports.some((r) => r.purged.length > 0);
  return {
    kind: anyPurged ? 'success' : 'info',
    message: `Project removed. ${perHost.join(' · ')}`,
  };
}
