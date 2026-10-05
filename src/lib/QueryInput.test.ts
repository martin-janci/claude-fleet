import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import QueryInput from './QueryInput.svelte';

const vocab = { hosts: ['local', 'oci'], layers: ['core'], catalogs: ['personal', 'papayapos'] };
const typed = async (input: HTMLElement, value: string) => fireEvent.input(input, { target: { value } });

describe('QueryInput', () => {
  it('completes a key, then its value; Tab and Enter take the highlighted one', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'ki' } });
    expect(screen.getByTestId('assets-query-completions').textContent).toContain('kind:');
    expect(input.getAttribute('aria-expanded')).toBe('true');
    await fireEvent.keyDown(input, { key: 'Tab' });
    expect(input.value).toBe('kind:');
    await fireEvent.input(input, { target: { value: 'kind:sk' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(input.value).toBe('kind:skill');
    expect(input.getAttribute('aria-expanded')).toBe('false');
  });

  it('moves the highlight with the arrows and names it for assistive tech', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'catalog:p' } });
    const first = input.getAttribute('aria-activedescendant');
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    const second = input.getAttribute('aria-activedescendant');
    expect(second).not.toBe(first);
    expect(document.getElementById(second!)?.getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(input.value).toBe('catalog:papayapos');
  });

  it('Esc closes the list, then clears, then hands focus back', async () => {
    const onescape = vi.fn();
    render(QueryInput, { vocab, onescape });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'ho' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByTestId('assets-query-completions')).toBeNull();
    expect(input.value).toBe('ho');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(input.value).toBe('');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(onescape).toHaveBeenCalledOnce();
  });

  it('is a labelled combobox over a listbox of options', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByRole('combobox', { name: 'Filter assets' });
    expect(input.getAttribute('aria-expanded')).toBe('false');
    expect(input.getAttribute('aria-autocomplete')).toBe('list');
    await typed(input, 'kind:s');
    const list = screen.getByRole('listbox');
    expect(input.getAttribute('aria-controls')).toBe(list.id);
    expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual(['kind:skill']);
    expect(screen.getAllByRole('option')[0].getAttribute('aria-selected')).toBe('true');
  });

  it('completes case-insensitively and keeps the earlier words', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await typed(input, 'infra HO');
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(input.value).toBe('infra host:');
    await typed(input, 'infra HOST:O');
    await fireEvent.keyDown(input, { key: 'Tab' });
    expect(input.value).toBe('infra host:oci');
  });

  it('offers at most eight completions', async () => {
    const wide = { hosts: Array.from({ length: 12 }, (_, i) => `h${i}`), layers: [], catalogs: [] };
    render(QueryInput, { vocab: wide });
    await typed(screen.getByTestId('assets-query'), 'host:');
    expect(screen.getAllByRole('option')).toHaveLength(8);
  });

  it('wraps the highlight with ArrowUp and ArrowDown, and takes a clicked option', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await typed(input, 'host:');
    const options = screen.getAllByRole('option');
    expect(options.map((o) => o.textContent)).toEqual(['host:local', 'host:oci']);
    await fireEvent.keyDown(input, { key: 'ArrowUp' });
    expect(options[1].getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    expect(options[0].getAttribute('aria-selected')).toBe('true');
    await fireEvent.mouseDown(options[1]);
    expect(input.value).toBe('host:oci');
  });

  it('with no list open, Tab and Enter are left alone and a closed list shows nothing', async () => {
    render(QueryInput, { vocab });
    const input = screen.getByTestId('assets-query') as HTMLInputElement;
    await typed(input, 'plain words');
    expect(screen.queryByRole('listbox')).toBeNull();
    const tab = new KeyboardEvent('keydown', { key: 'Tab', cancelable: true, bubbles: true });
    input.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(false);
  });

  it('shows the parsed tokens beside the field, and focus() focuses it', async () => {
    const { component } = render(QueryInput, { vocab, value: 'kind:skill scope:shared,org infra' });
    expect(screen.getByTestId('assets-query-tokens').textContent).toContain('kind:skill');
    expect(screen.getByTestId('assets-query-tokens').textContent).toContain('scope:shared,org');
    (component as unknown as { focus: () => void }).focus();
    expect(document.activeElement).toBe(screen.getByTestId('assets-query'));
  });
});
