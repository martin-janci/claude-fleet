// Redesign 10.12: wizards in the chat. An agent's form draws in while it is
// still written (`ask { draft }` on its session row); the app's Add project
// opens as a form at the end of a conversation (Control's chip), builds while
// it reads the hosts, runs only on its last button, then shrinks to a line.
// Never a dialog.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ChatWizards from './ChatWizards.svelte';
import { chatWizards, openChatWizard } from './chat_wizards';
import { hosts } from '../hosts';
import { host } from '../hosts_fixture';
import type { SessionRow } from '../sessions';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

type Row = Pick<SessionRow, 'id' | 'pending_form' | 'form_draft'>;
const row = (over: Partial<Row> = {}): Row => ({ id: 7, pending_form: null, form_draft: null, ...over });

const DRAFT = JSON.stringify({
  spec: 'fleet.form/1',
  title: 'Staging database',
  steps: [{ title: 'Where', fields: [{ name: 'engine', type: 'select', label: 'Engine', options: [['pg', 'Postgres']] }] }],
});

beforeEach(() => {
  inv.mockReset();
  chatWizards.set(new Map());
  hosts.set([host('local'), host('mercury')]);
});

describe('an agent’s form while it is written', () => {
  it('draws the title and each whole field in, with what the agent reads', () => {
    const text = DRAFT.slice(0, DRAFT.indexOf('"options"'));
    render(ChatWizards, { props: { session: row({ form_draft: { draft: text, why: 'your hosts', updated_at: Date.now() / 1000 } }), agentName: 'Claude' } });
    const card = screen.getByTestId('chat-form-draft');
    expect(within(card).getByTestId('chat-form-building')).toHaveTextContent('Building a form · reading your hosts');
    expect(within(card).getByTestId('chat-form-draft-title')).toHaveTextContent('Staging database');
    expect(within(card).getByTestId('chat-form-draft-engine')).toHaveTextContent('Engine');
  });

  it('stays a sketch even once it parses: the agent’s ask opens the form', () => {
    render(ChatWizards, { props: { session: row({ form_draft: { draft: DRAFT, why: null, updated_at: Date.now() / 1000 } }), agentName: 'Claude' } });
    expect(screen.getByTestId('chat-form-building')).toBeInTheDocument();
    expect(screen.queryByTestId('form-submit')).toBeNull();
  });

  it('gives way to the open form, and to an abandoned draft’s age', () => {
    const fresh = { draft: DRAFT, why: null, updated_at: Date.now() / 1000 };
    const { rerender } = render(ChatWizards, {
      props: { session: row({ form_draft: fresh, pending_form: { form_id: 'f_1', title: 'Staging database' } }), agentName: 'Claude' },
    });
    expect(screen.queryByTestId('chat-form-draft')).toBeNull();
    void rerender({ session: row({ form_draft: { ...fresh, updated_at: Date.now() / 1000 - 3600 } }), agentName: 'Claude' });
    expect(screen.queryByTestId('chat-form-draft')).toBeNull();
  });

  it('is not drawn on an earlier conversation', () => {
    render(ChatWizards, {
      props: { session: row({ form_draft: { draft: DRAFT, why: null, updated_at: Date.now() / 1000 } }), agentName: 'Claude', live: false },
    });
    expect(screen.queryByTestId('chat-form-draft')).toBeNull();
  });
});

describe('Add project from Control', () => {
  it('builds, opens with the hosts, runs only on its last button, then shrinks to one line', async () => {
    openChatWizard(7, 'add_project', { from: 'Control' });
    render(ChatWizards, { props: { session: row(), agentName: 'Claude' } });
    // Building while it reads the hosts, with the wizard's title.
    expect(screen.getByTestId('chat-form-building')).toHaveTextContent('reading your hosts');
    await waitFor(() => expect(screen.queryByTestId('chat-form-building')).toBeNull());
    const card = screen.getByTestId('chat-form');
    expect(card).toHaveTextContent('Add project');
    expect(card).toHaveTextContent('from Control');
    expect(card).toHaveTextContent('Nothing runs until you press Add project.');
    // No GitHub list in a form: From GitHub has left; the hosts are the fleet's.
    expect(screen.queryByTestId('form-field-source-github')).toBeNull();
    await fireEvent.click(screen.getByTestId('form-field-source-clone'));
    await fireEvent.click(screen.getByTestId('form-field-host-mercury'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-url'), { target: { value: 'acme/papaya-pos' } });
    expect(inv).not.toHaveBeenCalled();
    inv.mockResolvedValueOnce({
      project: { id: 3, owner: 'acme', repo: 'papaya-pos', base_path: '/p', last_session_at: null, adopted: false, system: false },
      worktrees: [],
    });
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(inv).toHaveBeenCalledWith('add_project', {
      args: expect.objectContaining({ host_alias: 'mercury', source: { kind: 'clone', url: 'acme/papaya-pos' } }),
    });
    const done = await screen.findByTestId('chat-form-outcome');
    expect(done.dataset.state).toBe('answered');
    expect(screen.getByTestId('chat-form-summary')).toHaveTextContent('acme/papaya-pos on mercury');
    expect(screen.queryByRole('dialog')).toBeNull();
    // Answered: a second open is a fresh card; Dismiss takes this one away.
    expect(get(chatWizards).get(7)?.[0].ended).toBe(true);
    await fireEvent.click(screen.getByTestId('chat-wizard-dismiss'));
    await waitFor(() => expect(screen.queryByTestId('chat-form-outcome')).toBeNull());
  });

  it('opening it twice keeps the one open card', () => {
    const a = openChatWizard(7, 'add_project');
    const b = openChatWizard(7, 'add_project');
    expect(a).toBe(b);
    expect(get(chatWizards).get(7)).toHaveLength(1);
  });
});
