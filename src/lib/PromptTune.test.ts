// The prompt tuner panel: tunes on Enter or a quick ask, shows the Drafted
// rewrite with its notes and its changes, and puts it in the box only on the
// person's "Use this" (edited first if they edited it); Discard drops it.
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import PromptTune, { _forgetLeftTunes } from './PromptTune.svelte';
import { toasts, clearToasts, runToastAction } from './toasts';
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
  beforeEach(() => {
    _forgetLeftTunes();
    clearToasts();
  });

  it('tunes on Enter and shows the drafted rewrite, its notes and its changes', async () => {
    const { tune, ask } = setup();
    await fireEvent.input(ask, { target: { value: 'say when done' } });
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    expect(tune).toHaveBeenCalledWith('say when done', 'Fix the failing login test.');
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
    expect(tune).toHaveBeenCalledWith('Shorter', 'Fix the failing login test.');
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
  // Review w07-3: Tune again and the chips build on the rewrite on screen.
  it('tunes the edited rewrite again, and the changes still compare against the draft', async () => {
    const { tune, ask } = setup();
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    await fireEvent.input(screen.getByTestId('prompt-tune-rewrite'), { target: { value: 'Mine now.' } });
    tune.mockResolvedValueOnce({ ok: true, value: { ...answer, prompt: 'Mine, shorter.' } });
    await fireEvent.click(screen.getAllByTestId('prompt-tune-quick')[0]);
    await flush();
    expect(tune).toHaveBeenLastCalledWith('Shorter', 'Mine now.');
    await fireEvent.click(screen.getByTestId('prompt-tune-view-changes'));
    const diff = screen.getByTestId('prompt-tune-diff').textContent ?? '';
    expect(diff).toContain('− Fix the failing login test.');
    expect(diff).toContain('+ Mine, shorter.');
  });

  it('a quick ask joins what was typed and leaves the field as typed', async () => {
    const { tune, ask } = setup();
    await fireEvent.input(ask, { target: { value: 'keep PAY-9' } });
    await fireEvent.click(screen.getAllByTestId('prompt-tune-quick')[0]);
    await flush();
    expect(tune).toHaveBeenCalledWith('keep PAY-9; Shorter', 'Fix the failing login test.');
    expect(ask.value).toBe('keep PAY-9');
  });

  // Review w07-2: the box's own undo cannot bring a replaced draft back.
  it('Use this offers Undo, which puts the draft back', async () => {
    const { onaccept, ask } = setup();
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    await fireEvent.click(screen.getByTestId('prompt-tune-accept'));
    const t = get(toasts).at(-1);
    expect(t?.message).toBe('Draft replaced');
    expect(t?.action?.label).toBe('Undo');
    runToastAction(t!.id);
    expect(onaccept).toHaveBeenLastCalledWith('Fix the failing login test.');
  });

  // Review w07-11.
  it('Mod+Enter in the rewrite uses it', async () => {
    const { onaccept, ask } = setup();
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    await fireEvent.keyDown(screen.getByTestId('prompt-tune-rewrite'), { key: 'Enter', ctrlKey: true });
    expect(onaccept).toHaveBeenCalledWith(answer.prompt);
    expect(screen.getByTestId('prompt-tune').getAttribute('aria-busy')).toBe('false');
  });

  // Review w07-4: the reason is in the panel, not only in a tooltip.
  it('says why it cannot tune in the panel', () => {
    setup({ draft: '' });
    expect(screen.getByTestId('prompt-tune-blocked').textContent).toMatch(/Write a draft first/);
  });

  // Review w07-7.
  it('a cut rewrite says so and its Use this is marked', async () => {
    const { ask } = setup({ reply: { ok: true, value: { ...answer, cut: true } } });
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    expect(screen.getByTestId('prompt-tune-cut')).toBeTruthy();
    expect(screen.getByTestId('prompt-tune-accept').textContent).toBe('Use this (cut)');
  });

  // Review w07-6: closing while it runs does not pay for a second run.
  it('Stop waiting keeps the run, and reopening on the same draft shows it without tuning again', async () => {
    let finish: (r: Result<TuneAnswer>) => void = () => {};
    const { tune, ask } = setup();
    tune.mockImplementationOnce(() => new Promise((res) => (finish = res)));
    await fireEvent.keyDown(ask, { key: 'Enter' });
    await flush();
    expect(screen.getByTestId('prompt-tune').getAttribute('aria-busy')).toBe('true');
    await fireEvent.click(screen.getByTestId('prompt-tune-stop'));
    await flush();
    expect(screen.queryByTestId('prompt-tune-stop')).toBeNull();
    cleanup();
    finish({ ok: true, value: answer });
    const again = setup();
    await flush();
    expect(again.tune).not.toHaveBeenCalled();
    expect((screen.getByTestId('prompt-tune-rewrite') as HTMLTextAreaElement).value).toBe(answer.prompt);
  });
});
