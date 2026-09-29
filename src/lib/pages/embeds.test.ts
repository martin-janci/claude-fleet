// Declarative pages L8: the embed pages the desktop's own screens draw
// (`embeds.generated.json`, generated from crates/fleet-core/pages). Every
// slot a spec fills is rendered by a screen, every rendered slot is filled,
// and each slot draws its views from the context its owner hands it.
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import EmbedSlot from './EmbedSlot.svelte';
import { RENDERED_SLOTS, embedItems, embedPages } from './embeds';
import type { Slot } from './pages';
import { NOW, WORK, fleetAccounts, fleetHosts, fleetUsage, host, snapshot } from '../hosts_fixture';

const SOURCES = import.meta.glob('../../**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

describe('embed pages', () => {
  it('fill exactly the slots the desktop renders, each once', () => {
    const filled = embedPages.map((p) => p.slot).sort();
    expect(filled).toEqual([...RENDERED_SLOTS].sort());
  });

  it('every rendered slot has an owner that draws it', () => {
    for (const slot of RENDERED_SLOTS) {
      const owners = Object.entries(SOURCES)
        .filter(([, src]) => src.includes(`slot="${slot}"`))
        .map(([path]) => path);
      expect(owners, slot).toHaveLength(1);
    }
  });

  it('hold only account_usage items that read the live source', () => {
    for (const slot of RENDERED_SLOTS) {
      for (const item of embedItems(slot)) {
        expect(item.type, slot).toBe('account_usage');
        if (item.type === 'account_usage') expect(item.source.id).toBe('accounts.usage');
      }
    }
  });
});

describe('EmbedSlot', () => {
  const work = snapshot(WORK.uuid);
  const base = { now: NOW, locale: 'en-GB', timeZone: 'UTC' };

  const cases: [Slot, Record<string, unknown>, string[]][] = [
    ['host_detail', { account: WORK, snapshot: work, onrefresh: () => {} }, ['usage-block', 'usage-refresh']],
    ['hosts_group_title', { account: WORK, snapshot: work }, ['group-freshness']],
    ['hosts_group', { account: WORK, snapshot: work }, ['group-usage-5h', 'group-usage-weekly']],
    ['new_session_chip', { host: host('mefistos', { account_uuid: WORK.uuid }), account: WORK, snapshot: work }, ['chip-usage']],
    [
      'new_session_host',
      { host: host('mefistos', { account_uuid: WORK.uuid }), hosts: fleetHosts(), account: WORK, snapshot: work },
      ['host-usage-line'],
    ],
    [
      'status_footer',
      { hosts: fleetHosts(), accounts: fleetAccounts(), snapshots: fleetUsage() },
      ['footer-usage'],
    ],
  ];

  it.each(cases)('%s draws its views from its context', (slot, ctx, testids) => {
    render(EmbedSlot, { props: { slot, ctx: { ...base, ...ctx } } });
    for (const id of testids) expect(screen.getByTestId(id), `${slot}: ${id}`).toBeInTheDocument();
  });

  it('the footer opens the Hosts view at the host it names', async () => {
    const onopenhost = vi.fn();
    render(EmbedSlot, {
      props: {
        slot: 'status_footer',
        ctx: { ...base, hosts: fleetHosts(), accounts: fleetAccounts(), snapshots: fleetUsage(), onopenhost },
      },
    });
    screen.getByTestId('footer-usage').click();
    expect(onopenhost).toHaveBeenCalledTimes(1);
  });

  it('the host detail block has no refresh without an account', () => {
    render(EmbedSlot, { props: { slot: 'host_detail', ctx: { ...base, account: null, snapshot: null, onrefresh: () => {} } } });
    expect(screen.getByTestId('usage-block')).toBeInTheDocument();
    expect(screen.queryByTestId('usage-refresh')).toBeNull();
  });
});
