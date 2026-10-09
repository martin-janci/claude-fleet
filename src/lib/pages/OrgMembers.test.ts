// Redesign 11.7c: an admin sees that a member's private sessions exist, as a
// count beside their name, never which ones.
import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import OrgMembers from './OrgMembers.svelte';
import { memberSessionsWord, type OrgMember } from '../orgs';
import type { FieldSpec, ResourceRecord } from './resources';
import { bundle } from './testing';
import { expectAccessible } from '../a11y_check';

const members: OrgMember[] = [
  { person_id: 2, name: 'martin', role: 'admin', live_sessions: 6 },
  { person_id: 3, name: 'jana', role: 'member', live_sessions: 1, private_sessions: 1 },
  { person_id: 4, name: 'peter', role: 'member' },
];

function mount(list: OrgMember[] = members) {
  return render(OrgMembers, {
    props: {
      record: { id: 1, name: 'Acme' } as unknown as ResourceRecord,
      members: list,
      readonly: true,
      options: () => [],
      run: async () => true,
    },
  });
}

describe('OrgMembers: sessions (11.7c)', () => {
  it('words the counts, and a private one as existing only', () => {
    expect(memberSessionsWord({})).toBe('—');
    expect(memberSessionsWord({ live_sessions: 6 })).toBe('6 live');
    expect(memberSessionsWord({ live_sessions: 1, private_sessions: 1 })).toBe('1 live, private');
    expect(memberSessionsWord({ live_sessions: 3, private_sessions: 2 })).toBe('3 live, 2 private');
  });

  it('the New layout shows each member’s sessions; private ones say why', () => {
    mount();
    const cells = screen.getAllByTestId('member-sessions');
    expect(cells.map((c) => c.textContent)).toEqual(['6 live', '1 live, private', '—']);
    expect(cells[1].getAttribute('title')).toContain('exists, not what it is');
    expect(cells[0].getAttribute('title')).toBeNull();
  });

  it('an older hub that sends no counts shows no column', () => {
    mount(members.map(({ person_id, name, role }) => ({ person_id, name, role })));
    expect(screen.queryByTestId('member-sessions')).toBeNull();
  });

  it('the member list, editable, in the New layout, is accessible', async () => {
    const field = bundle.resources
      .flatMap((r) => r.fields)
      .find((f) => f.type === 'items' && f.item_label.type === 'member') as Extract<
      FieldSpec,
      { type: 'items' }
    >;
    const { container } = render(OrgMembers, {
      props: {
        record: { id: 1, name: 'Acme' } as unknown as ResourceRecord,
        members: members.map((m) => ({ ...m, devices: ['laptop'] })),
        addAction: field.add[0],
        removeAction: field.remove,
        readonly: false,
        options: () => [
          { value: 'member', label: 'Member' },
          { value: 'admin', label: 'Admin' },
        ],
        run: async () => true,
      },
    });
    expect(screen.getAllByTestId('item-remove-members')).toHaveLength(3);
    await expectAccessible(container);
  });
});
