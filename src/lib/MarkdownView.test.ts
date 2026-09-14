import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock('./clipboard', () => ({ copyText: vi.fn(() => Promise.resolve(true)) }));

import { openUrl } from '@tauri-apps/plugin-opener';
import { copyText } from './clipboard';
import Markdown from './MarkdownView.svelte';

const mockedOpen = openUrl as unknown as ReturnType<typeof vi.fn>;
const mockedCopy = copyText as unknown as ReturnType<typeof vi.fn>;

beforeEach(() => {
  mockedOpen.mockClear();
  mockedCopy.mockClear();
});

describe('Markdown', () => {
  it('renders headings, emphasis, lists, quotes, tables and rules as elements', () => {
    const { container } = render(Markdown, {
      source: [
        '## Plan',
        '',
        'Some **bold**, *em*, ~~old~~ and `code`.',
        '',
        '1. first',
        '2. second',
        '',
        '- [x] done',
        '',
        '> quoted',
        '',
        '| a | b |',
        '|---|--:|',
        '| 1 | 2 |',
        '',
        '---',
      ].join('\n'),
    });
    expect(container.querySelector('.md-h2')?.textContent).toBe('Plan');
    expect(container.querySelector('strong')?.textContent).toBe('bold');
    expect(container.querySelector('em')?.textContent).toBe('em');
    expect(container.querySelector('del')?.textContent).toBe('old');
    expect(container.querySelector('code.md-code')?.textContent).toBe('code');
    expect(container.querySelectorAll('ol li')).toHaveLength(2);
    const task = container.querySelector('input[type="checkbox"]') as HTMLInputElement;
    expect(task.checked).toBe(true);
    expect(task.disabled).toBe(true);
    expect(container.querySelector('blockquote')?.textContent).toContain('quoted');
    expect(container.querySelector('td:last-child')?.getAttribute('style')).toContain('text-align: right');
    expect(container.querySelector('hr')).not.toBeNull();
  });

  it('never turns raw HTML into elements', () => {
    const { container } = render(Markdown, {
      source: '<script>window.pwned = 1</script>\n\n<img src=x onerror="window.pwned=1"> **<b>x</b>**',
    });
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('b')).toBeNull();
    expect(container.textContent).toContain('<script>window.pwned = 1</script>');
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
  });

  it('opens safe links in the system browser without navigating', async () => {
    render(Markdown, { source: 'See [the PR](https://github.com/a/b/pull/1).' });
    const link = screen.getByText('the PR').closest('a')!;
    expect(link.getAttribute('href')).toBe('https://github.com/a/b/pull/1');
    const click = new MouseEvent('click', { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    await Promise.resolve();
    expect(mockedOpen).toHaveBeenCalledWith('https://github.com/a/b/pull/1');
  });

  it('renders unsafe links as inert text', () => {
    const { container } = render(Markdown, { source: '[click](javascript:alert(1)) and [f](file:///etc/passwd)' });
    expect(container.querySelector('a')).toBeNull();
    expect(container.querySelectorAll('.md-link-inert')).toHaveLength(2);
  });

  it('highlights fenced code and copies its raw text', async () => {
    const { container } = render(Markdown, { source: '```ts\nconst x = "hi"; // note\n```' });
    const pre = container.querySelector('pre.md-pre')!;
    expect(pre.textContent).toBe('const x = "hi"; // note');
    expect(pre.querySelector('.tok-kw')?.textContent).toBe('const');
    expect(pre.querySelector('.tok-str')).not.toBeNull();
    expect(container.querySelector('.md-lang')?.textContent).toBe('ts');
    await fireEvent.click(screen.getByTestId('md-copy'));
    expect(mockedCopy).toHaveBeenCalledWith('const x = "hi"; // note');
    await tick();
    expect(screen.getByTestId('md-copy').textContent).toBe('Copied');
  });

  it('keeps multi-line code line breaks', () => {
    const { container } = render(Markdown, { source: '```\na\n\nb\n```' });
    expect(container.querySelector('pre.md-pre')!.textContent).toBe('a\n\nb');
  });
});
