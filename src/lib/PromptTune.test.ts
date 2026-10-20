// The prompt tuner panel: tunes on Enter or a quick ask, shows the Drafted
// rewrite with its notes and its changes, and puts it in the box only on the
// person's "Use this" (edited first if they edited it); Discard drops it.
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';
import PromptTune from './PromptTune.svelte';
import type { TuneAnswer } from './prompt_tune';
import type { Result } from './result';

const answer: TuneAnswer = {
  prompt: 'Fix the failing login test.\nDone when cargo test passes.',
  notes: 'Said when it is done.',
  unchanged: false,
  model: 'haiku',
  host_alias: 'mercury',
  at: 1,
};

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

function setup(over: Partial<{ draft: string; reply: Result<TuneAnswer> }> = {}) {
  const tune = vi.fn(async () => over.reply ?? ({ ok: true, value: answer } as Result<TuneAnswer>));
  const onaccept = vi.fn();
  const onclose = vi.fn();
  const { rerender } = render(PromptTune, { model: 'haiku', draft: over.draft ?? 'Fix the failing login test.', tune, onaccept, onclose });
  return { tune, onaccept, onclose, rerender, ask: screen.getByTestId('prompt-tune-ask') as HTMLInputElement };
}

describe('PromptTune', () => {
  it('tunes on Enter and shows the drafted rewrite, its notes and its changes', async () => {
    const { tune, ask } = setup();
    await fireEvent.input(ask, { target: { value: 'say when done' } });
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    expect(tune).toHaveBeenCalledWith('say when done');
    expect((screen.getByTestId('prompt-tune-rewrite') as HTMLTextAreaElement).value).toBe(answer.prompt);
    expect(screen.getByTestId('prompt-tune-notes').textContent).toBe(answer.notes);
    expect(screen.getByTestId('prompt-tune-drafted').textContent).toBe('Drafted');
    expect(screen.getByTestId('prompt-tune-meta').textContent).toBe('by haiku on mercury');
    await fireEvent.click(screen.getByTestId('prompt-tune-view-changes'));
    const diff = screen.getByTestId('prompt-tune-diff').textContent ?? '';
    expect(diff).toContain('+ Done when cargo test passes.');
    expect(diff).toContain('  Fix the failing login test.');
  });

  it('a quick ask tunes with its words', async () => {
    const { tune } = setup();
    await fireEvent.click(screen.getAllByTestId('prompt-tune-quick')[0]);
    await flush();
    expect(tune).toHaveBeenCalledWith('Shorter');
  });

  it('puts the edited rewrite in the box only on Use this; Discard drops it', async () => {
    const { onaccept, onclose, ask } = setup();
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    await fireEvent.input(screen.getByTestId('prompt-tune-rewrite'), { target: { value: 'Mine now.' } });
    expect(onaccept).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('prompt-tune-accept'));
    expect(onaccept).toHaveBeenCalledWith('Mine now.');
    expect(onclose).toHaveBeenCalled();

    cleanup();
    const again = setup();
    await fireEvent.keyDown(again.ask, { key: 'Enter' });
    await flush();
    await fireEvent.click(screen.getByTestId('prompt-tune-discard'));
    expect(again.onaccept).not.toHaveBeenCalled();
    expect(again.onclose).toHaveBeenCalled();
  });

  it('an empty draft cannot be tuned, and an error is shown', async () => {
    setup({ draft: '  ' });
    expect((screen.getByTestId('prompt-tune-run') as HTMLButtonElement).disabled).toBe(true);
    cleanup();
    const { ask } = setup({ reply: { ok: false, error: { code: 'E_INVALID_STATE', message: 'Prompt tuner is off.' } } });
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    expect(screen.getByTestId('prompt-tune-error').textContent).toBe('Prompt tuner is off.');
  });

  it('says so when the draft needed no change, and offers nothing to accept', async () => {
    const { ask } = setup({ reply: { ok: true, value: { ...answer, unchanged: true, notes: '' } } });
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    expect(screen.getByTestId('prompt-tune-unchanged')).toBeTruthy();
    expect(screen.queryByTestId('prompt-tune-accept')).toBeNull();
  });

  it('Esc on any of its buttons closes the tuner and goes no further', async () => {
    const { onclose, ask } = setup();
    const outer = vi.fn();
    document.body.addEventListener('keydown', outer);
    try {
      await fireEvent.keyDown(ask, { key: 'Enter' });
      await flush();
      for (const id of ['prompt-tune-accept', 'prompt-tune-discard', 'prompt-tune-close', 'prompt-tune-run']) {
        await fireEvent.keyDown(screen.getByTestId(id), { key: 'Escape' });
      }
      await fireEvent.keyDown(screen.getAllByTestId('prompt-tune-quick')[0], { key: 'Escape' });
      expect(onclose).toHaveBeenCalledTimes(5);
      expect(outer).not.toHaveBeenCalled();
    } finally {
      document.body.removeEventListener('keydown', outer);
    }
  });

  it('will not use a rewrite once the draft in the box has changed since the tune', async () => {
    const { onaccept, ask, rerender } = setup();
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    expect(screen.queryByTestId('prompt-tune-stale')).toBeNull();
    await rerender({ draft: 'Fix the failing login test, and the signup one.' });
    await flush();
    expect(screen.getByTestId('prompt-tune-stale')).toBeTruthy();
    const use = screen.getByTestId('prompt-tune-accept') as HTMLButtonElement;
    expect(use.disabled).toBe(true);
    await fireEvent.click(use);
    expect(onaccept).not.toHaveBeenCalled();
  });
});
