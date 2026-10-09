// Redesign 10.12: an agent (Control's operator or any session) opens one of
// the app's wizards in its chat with a `wizard` block. The card is the app's
// spec, never the block's; the last button runs it; how it went fills the
// composer for the agent, unsent.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import RichText from '../RichText.svelte';
import { composerDrafts } from '../conversation';
import { hosts } from '../hosts';
import { host } from '../hosts_fixture';
import { checkUiBlock } from '../rich_blocks';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const ui = (o: Record<string, unknown>) => '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', ...o }) + '\n```';

beforeEach(() => {
  inv.mockReset();
  composerDrafts.clear();
  hosts.set([host('local'), host('mercury')]);
});

describe('a wizard block', () => {
  it('names only the wizards the chat can run', () => {
    expect(checkUiBlock(JSON.stringify({ spec: 'fleet.ui/1', kind: 'wizard', wizard: 'get_started' })).ok).toBe(true);
    expect(checkUiBlock(JSON.stringify({ spec: 'fleet.ui/1', kind: 'wizard', wizard: 'link_hub' })).ok).toBe(false);
  });

  it('opens Add project as the app’s form, and fills the composer once it ran', async () => {
    render(RichText, { props: { source: ui({ kind: 'wizard', wizard: 'add_project', why: 'You asked for the receipts repo.' }), sessionId: 4 } });
    const card = await screen.findByTestId('wizard-chat-card');
    expect(card.dataset.wizard).toBe('add_project');
    await waitFor(() => expect(screen.getByTestId('chat-form')).toHaveTextContent('from the agent'));
    expect(screen.getByTestId('chat-form')).toHaveTextContent('You asked for the receipts repo.');
    await fireEvent.click(screen.getByTestId('form-field-source-folder'));
    await fireEvent.click(screen.getByTestId('form-field-host-local'));
    await fireEvent.click(screen.getByTestId('form-next'));
    await fireEvent.input(screen.getByTestId('form-field-path'), { target: { value: '~/src/pos' } });
    inv.mockResolvedValueOnce({ project: { id: 2, owner: 'local', repo: 'pos', base_path: '~/src/pos', last_session_at: null, adopted: false, system: false }, worktrees: [] });
    await fireEvent.click(screen.getByTestId('form-submit'));
    await screen.findByTestId('chat-form-outcome');
    expect(composerDrafts.get(4)).toBe('Done in the "Add project" form: local/pos on local');
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('is a line, not a form, where the composer is not on screen', () => {
    render(RichText, { props: { source: ui({ kind: 'wizard', wizard: 'new_session' }), sessionId: null } });
    expect(screen.getByTestId('rich-wizard-off')).toHaveTextContent('New session: opens in the live conversation.');
    expect(screen.queryByTestId('chat-form')).toBeNull();
  });
});
