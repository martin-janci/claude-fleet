// Autofix in the app: the findings store, the shared FixCard (Fix asks
// first and lists what changes, Later hides, a login when fleet has no fix)
// and the `fix` chat card, which draws only what the hub's row says.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FixCard from './FixCard.svelte';
import RichText from './RichText.svelte';
import {
  canApplyFixes,
  findings,
  findingsByHost,
  findingsError,
  findingsLoaded,
  fixedRecently,
  loadFindings,
  operatorFinding,
  stateWords,
  subjectWords,
  type Finding,
} from './autofix';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const finding = (over: Partial<Finding> = {}): Finding => ({
  id: 7,
  kind: 'host_mcp_entry.token',
  subject: 'host:mercury',
  host_alias: 'mercury',
  severity: 'degraded',
  state: 'open',
  first_seen_at: 1,
  last_seen_at: 2,
  attempts: 0,
  title: 'Claude Code on mercury signs in to fleet with the wrong token',
  explanation: 'Fleet tells hosts apart by their token.',
  evidence: ["it carries hetzner's host token (sha256 abcdef012345…), not mercury's"],
  fix: {
    summary: "Repair fleet's entry in mercury's ~/.claude.json",
    changes: ['Rewrites the `claude-fleet` server.', 'Keeps every other key.'],
    safety: 'needs_approval',
    reversible: true,
    auto: false,
  },
  ...over,
});

let live: Finding[] = [];
let fixed: Finding[] = [];
let canApply = true;
let calls: { cmd: string; args: unknown }[] = [];

beforeEach(() => {
  live = [];
  fixed = [];
  canApply = true;
  calls = [];
  findings.set([]);
  fixedRecently.set([]);
  findingsLoaded.set(false);
  findingsError.set(null);
  canApplyFixes.set(false);
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, args: unknown) => {
    calls.push({ cmd, args });
    if (cmd === 'autofix_findings') return { findings: live, fixed, can_apply: canApply };
    if (cmd === 'autofix_apply') {
      const id = (args as { id: number }).id;
      const f = live.find((x) => x.id === id)!;
      return { ...f, state: 'cleared', attempts: 1 };
    }
    if (cmd === 'autofix_dismiss') {
      const id = (args as { id: number }).id;
      return { ...live.find((x) => x.id === id)!, state: 'dismissed', dismissed_until: 99 };
    }
    return null;
  });
});

describe('the findings store', () => {
  it('loads, counts open findings per host and picks out Control’s own', async () => {
    live = [
      finding(),
      finding({ id: 8, kind: 'host.hooks_silent' }),
      finding({ id: 9, state: 'dismissed' }),
      finding({ id: 10, kind: 'operator_unbound', subject: 'token:ux-agent', host_alias: null }),
    ];
    await loadFindings();
    expect(get(findingsByHost).get('mercury')).toBe(2);
    expect(get(operatorFinding)?.id).toBe(10);
    expect(get(canApplyFixes)).toBe(true);
    expect(get(findingsLoaded)).toBe(true);
  });

  it('words subjects and states', () => {
    expect(subjectWords('host:mercury')).toBe('host mercury');
    expect(subjectWords('session:12')).toBe('session #12');
    expect(stateWords(finding({ state: 'apply_failed', last_error: 'boom' }))).toBe('The fix did not work: boom');
    expect(stateWords(finding())).toBeNull();
  });
});

describe('FixCard', () => {
  it('asks first, listing what the fix changes, then applies once', async () => {
    live = [finding()];
    await loadFindings();
    render(FixCard, { props: { finding: live[0] } });
    expect(screen.getByTestId('fix-card-title').textContent).toContain('wrong token');
    await fireEvent.click(screen.getByTestId('fix-card-fix'));
    const changes = await screen.findByTestId('fix-card-changes');
    expect(changes.textContent).toContain('Rewrites the `claude-fleet` server.');
    expect(changes.textContent).toContain('Keeps every other key.');
    expect(calls.filter((c) => c.cmd === 'autofix_apply')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('fix-card-confirm'));
    await waitFor(() => expect(calls.filter((c) => c.cmd === 'autofix_apply')).toEqual([{ cmd: 'autofix_apply', args: { id: 7 } }]));
    await waitFor(() => expect(get(findings)).toHaveLength(0));
    expect(get(fixedRecently).map((f) => f.id)).toEqual([7]);
  });

  it('offers no Fix or Later on a device that may not apply', async () => {
    live = [finding()];
    canApply = false;
    await loadFindings();
    render(FixCard, { props: { finding: live[0] } });
    expect(screen.queryByTestId('fix-card-fix')).toBeNull();
    expect(screen.queryByTestId('fix-card-later')).toBeNull();
    expect(screen.getByTestId('fix-card-readonly')).toBeTruthy();
  });

  it('Later hides it for a day', async () => {
    live = [finding()];
    await loadFindings();
    render(FixCard, { props: { finding: live[0] } });
    await fireEvent.click(screen.getByTestId('fix-card-later'));
    await waitFor(() => expect(calls.find((c) => c.cmd === 'autofix_dismiss')?.args).toEqual({ id: 7, until: null }));
    await waitFor(() => expect(get(findings)[0].state).toBe('dismissed'));
  });

  it('Details shows the evidence', async () => {
    render(FixCard, { props: { finding: finding(), compact: true } });
    expect(screen.queryByTestId('fix-card-explanation')).toBeNull();
    await fireEvent.click(screen.getByTestId('fix-card-details-btn'));
    expect(screen.getByTestId('fix-card-details').textContent).toContain("hetzner's host token");
    expect(screen.getByTestId('fix-card-explanation')).toBeTruthy();
  });

  it('offers a login when fleet has no fix', () => {
    render(FixCard, {
      props: { finding: finding({ kind: 'stuck.auth_menu', fix: null, action: { action: 'login', host: 'mac' } }) },
    });
    expect(screen.getByTestId('fix-card-login').textContent).toContain('Log in on mac');
    expect(screen.queryByTestId('fix-card-fix')).toBeNull();
  });
});

const ui = (o: Record<string, unknown>) => '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', kind: 'fix', ...o }) + '\n```';

describe('the fix chat card', () => {
  it('draws the finding from the hub, not from the block', async () => {
    live = [finding({ id: 42 })];
    render(RichText, { props: { source: ui({ finding_id: 42, title: 'An invented fix' }), sessionId: 1 } });
    await screen.findByTestId('rich-fix');
    expect(screen.getByTestId('fix-card-title').textContent).toContain('wrong token');
    expect(screen.queryByText('An invented fix')).toBeNull();
  });

  it('is one quiet line once the finding is resolved or unknown', async () => {
    render(RichText, { props: { source: ui({ finding_id: 5 }), sessionId: 1 } });
    const line = await screen.findByTestId('rich-fix-resolved');
    expect(line.textContent).toContain('Already resolved');
  });

  it('says the read failed rather than calling the problem resolved', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'autofix_findings') throw { code: 'E_HUB_UNREACHABLE', message: 'hub unreachable' };
      return null;
    });
    render(RichText, { props: { source: ui({ finding_id: 5 }), sessionId: 1 } });
    const line = await screen.findByTestId('rich-fix-error');
    expect(line.textContent).toContain('Could not read the problem');
    expect(screen.queryByTestId('rich-fix-resolved')).toBeNull();
  });

  it('says what fleet fixed', async () => {
    fixed = [finding({ id: 6, state: 'cleared', title: 'Fleet’s files on h1 are out of date' })];
    render(RichText, { props: { source: ui({ finding_id: 6 }), sessionId: 1 } });
    const line = await screen.findByTestId('rich-fix-resolved');
    expect(line.textContent).toContain('Fixed: Fleet’s files on h1 are out of date');
  });
});
