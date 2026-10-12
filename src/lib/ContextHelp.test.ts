// The context help panel: asks on Enter, shows the Drafted answer, puts the
// proposal on the line only on the person's press (and only when it may),
// and recalls earlier questions with ↑.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';
import ContextHelp, { askedQuestions } from './ContextHelp.svelte';
import type { HelpAnswer } from './context_help';
import type { Result } from './result';

const answer: HelpAnswer = {
  answer: 'The branch has no upstream.',
  command: 'git push -u origin HEAD',
  model: 'haiku',
  host_alias: 'mercury',
  history_items: 12,
  at: 1,
};

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

function setup(over: Partial<{ line: string; insertBlocked: string | null; reply: Result<HelpAnswer> }> = {}) {
  const ask = vi.fn(async () => over.reply ?? ({ ok: true, value: answer } as Result<HelpAnswer>));
  const oninsert = vi.fn();
  const onclose = vi.fn();
  render(ContextHelp, {
    model: 'haiku',
    line: over.line ?? 'git push',
    what: 'shell 1’s history',
    ask,
    oninsert,
    onclose,
    insertBlocked: over.insertBlocked ?? null,
  });
  return { ask, oninsert, onclose, input: screen.getByTestId('context-help-question') as HTMLInputElement };
}

describe('ContextHelp', () => {
  it('asks on Enter and shows the drafted answer with its proposal', async () => {
    const { ask, oninsert, onclose, input } = setup();
    await fireEvent.input(input, { target: { value: 'why does it fail?' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(ask).toHaveBeenCalledWith('why does it fail?');
    expect(screen.getByTestId('context-help-answer').textContent).toBe(answer.answer);
    expect(screen.getByTestId('context-help-command').textContent).toBe(answer.command);
    expect(screen.getByTestId('context-help-drafted').textContent).toBe('Drafted');
    expect(screen.getByTestId('context-help-meta').textContent).toBe('by haiku on mercury · 12 lines of history');
    expect(oninsert).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('context-help-insert'));
    expect(oninsert).toHaveBeenCalledWith(answer.command);
    expect(onclose).toHaveBeenCalled();
  });

  it('asks about the line alone, and not at all with neither', async () => {
    const { ask } = setup({ line: '' });
    const button = screen.getByTestId('context-help-ask') as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    await fireEvent.click(button);
    expect(ask).not.toHaveBeenCalled();
  });

  it('shows why it failed', async () => {
    setup({ reply: { ok: false, error: { code: 'E_CLAUDE_CLI', message: 'claude is not on mercury’s login PATH' } } });
    await fireEvent.click(screen.getByTestId('context-help-ask'));
    await flush();
    expect(screen.getByTestId('context-help-error').textContent).toContain('login PATH');
  });

  it('keeps Insert off while a program runs in the shell', async () => {
    const { oninsert } = setup({ insertBlocked: 'vim is running in this shell: copy the command instead' });
    await fireEvent.click(screen.getByTestId('context-help-ask'));
    await flush();
    const insert = screen.getByTestId('context-help-insert') as HTMLButtonElement;
    expect(insert.disabled).toBe(true);
    await fireEvent.click(insert);
    expect(oninsert).not.toHaveBeenCalled();
  });

  it('closes on Escape and recalls earlier questions with ↑', async () => {
    const { onclose, input } = setup();
    await fireEvent.input(input, { target: { value: 'first question' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(askedQuestions().at(-1)).toBe('first question');
    await fireEvent.input(input, { target: { value: '' } });
    await fireEvent.keyDown(input, { key: 'ArrowUp' });
    expect(input.value).toBe('first question');
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    expect(input.value).toBe('');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(onclose).toHaveBeenCalled();
  });
});
