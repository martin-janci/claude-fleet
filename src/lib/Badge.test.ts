import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import Badge from './Badge.svelte';

describe('Badge', () => {
  it('says its state in words, the glyph only reinforcing it', () => {
    render(Badge, { label: '2 drifted', tone: 'warn', glyph: '◐', testid: 'b' });
    const b = screen.getByTestId('b');
    expect(b.textContent).toBe('◐2 drifted');
    expect(b.className).toContain('badge');
    expect(b.className).toContain('warn');
    expect(b.querySelector('.glyph')?.getAttribute('aria-hidden')).toBe('true');
  });

  it('marks a private scope by its border shape, not by colour alone', () => {
    render(Badge, { label: 'private', dashed: true, title: 'Personal catalog, private', testid: 'b' });
    const b = screen.getByTestId('b');
    expect(b.className).toContain('dashed');
    expect(b.className).toContain('neutral');
    expect(b.getAttribute('title')).toBe('Personal catalog, private');
  });

  it('is information, never a control', () => {
    render(Badge, { label: 'orphan', tone: 'warn', testid: 'b' });
    const b = screen.getByTestId('b');
    expect(b.tagName).toBe('SPAN');
    expect(b.getAttribute('role')).toBeNull();
    expect(b.getAttribute('tabindex')).toBeNull();
  });
});
