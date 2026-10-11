import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import LayerChangeForm from './LayerChangeForm.svelte';
import { orgs, type OrgDetail } from './orgs';

describe('LayerChangeForm', () => {
  it('creates a context layer in the chosen catalog', async () => {
    const onsubmit = vi.fn();
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal', 'acme'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'servers' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenCalledWith({ op: 'create', catalog: 'personal', layer: 'servers', axis: 'context' });
  });
  it('creates a role layer in another catalog', async () => {
    const onsubmit = vi.fn();
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal', 'acme'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    await fireEvent.change(screen.getByTestId('layer-form-catalog'), { target: { value: 'acme' } });
    await fireEvent.change(screen.getByTestId('layer-form-axis'), { target: { value: 'role' } });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'ops-2' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenCalledWith({ op: 'create', catalog: 'acme', layer: 'ops-2', axis: 'role' });
  });
  it('a layer can apply by organisation, to one org, as a context layer (G7.5)', async () => {
    orgs.set([]);
    const onsubmit = vi.fn();
    const { unmount } = render(LayerChangeForm, { mode: 'create', catalogs: ['personal'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    expect(screen.getByTestId('layer-form-by-org')).toBeDisabled();
    unmount();
    orgs.set([{ name: 'Papaya' }, { name: 'Acme' }] as OrgDetail[]);
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    await fireEvent.click(screen.getByTestId('layer-form-by-org'));
    expect(screen.queryByTestId('layer-form-axis')).toBeNull();
    await fireEvent.change(screen.getByTestId('layer-form-org'), { target: { value: 'Acme' } });
    expect(screen.getByTestId('layer-form-org-note').textContent).toContain('every host of Acme');
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'acme-tools' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenCalledWith({ op: 'create', catalog: 'personal', layer: 'acme-tools', axis: 'context', orgs: ['Acme'] });
    orgs.set([]);
  });
  it('refuses an invalid name before submitting', async () => {
    const onsubmit = vi.fn();
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'Bad Name' } });
    expect(screen.getByTestId('layer-form-submit')).toBeDisabled();
    expect(screen.getByTestId('layer-form')).toHaveTextContent('lowercase letters, digits and dashes');
  });
  it('holds the backend’s rule: [a-z0-9][a-z0-9-]*', async () => {
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal'], catalog: 'personal', onsubmit: vi.fn(), oncancel: vi.fn() });
    const name = screen.getByTestId('layer-form-name');
    const submit = screen.getByTestId('layer-form-submit');
    expect(submit).toBeDisabled(); // empty
    for (const bad of ['-core', 'core_x', 'Core', 'a b', 'core ']) {
      await fireEvent.input(name, { target: { value: bad } });
      expect(submit, bad).toBeDisabled();
    }
    for (const good of ['core', '9lives', 'a-b-c', 'x']) {
      await fireEvent.input(name, { target: { value: good } });
      expect(submit, good).toBeEnabled();
    }
  });
  it('renames and moves', async () => {
    const onsubmit = vi.fn();
    const { unmount } = render(LayerChangeForm, { mode: 'rename', catalogs: ['personal'], catalog: 'personal', layer: 'core', onsubmit, oncancel: vi.fn() });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'base' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenLastCalledWith({ op: 'rename', catalog: 'personal', layer: 'core', to: 'base' });
    unmount();
    render(LayerChangeForm, { mode: 'move', catalogs: ['personal'], catalog: 'personal', layer: 'core', member: 'skill/w', layers: ['core', 'extra'], onsubmit, oncancel: vi.fn() });
    await fireEvent.change(screen.getByTestId('layer-form-to'), { target: { value: 'extra' } });
    await fireEvent.click(screen.getByTestId('layer-form-submit'));
    expect(onsubmit).toHaveBeenLastCalledWith({ op: 'move', catalog: 'personal', member: 'skill/w', layer: 'core', to: 'extra' });
  });
  it('offers every layer but the current one to move to', () => {
    render(LayerChangeForm, { mode: 'move', catalogs: ['personal'], catalog: 'personal', layer: 'core', member: 'skill/w', layers: ['core', 'extra', 'more'], onsubmit: vi.fn(), oncancel: vi.fn() });
    const opts = Array.from(screen.getByTestId('layer-form-to').querySelectorAll('option')).map((o) => o.value);
    expect(opts).toEqual(['extra', 'more']);
  });
  it('a rename to the same name, or a move with nowhere to go, cannot be submitted', async () => {
    const { unmount } = render(LayerChangeForm, { mode: 'rename', catalogs: ['personal'], catalog: 'personal', layer: 'core', onsubmit: vi.fn(), oncancel: vi.fn() });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'core' } });
    expect(screen.getByTestId('layer-form-submit')).toBeDisabled();
    unmount();
    render(LayerChangeForm, { mode: 'move', catalogs: ['personal'], catalog: 'personal', layer: 'core', member: 'skill/w', layers: ['core'], onsubmit: vi.fn(), oncancel: vi.fn() });
    expect(screen.getByTestId('layer-form-submit')).toBeDisabled();
  });
  it('Propose is the one primary, Cancel is quiet and calls oncancel; busy disables Propose', async () => {
    const oncancel = vi.fn();
    const { unmount } = render(LayerChangeForm, { mode: 'rename', catalogs: ['personal'], catalog: 'personal', layer: 'core', onsubmit: vi.fn(), oncancel });
    const primaries = document.querySelectorAll('.btn--primary');
    expect(primaries).toHaveLength(1);
    expect(primaries[0].textContent).toBe('Propose');
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toHaveClass('btn--quiet');
    await fireEvent.click(cancel);
    expect(oncancel).toHaveBeenCalled();
    unmount();
    render(LayerChangeForm, { mode: 'rename', catalogs: ['personal'], catalog: 'personal', layer: 'core', busy: true, onsubmit: vi.fn(), oncancel });
    await fireEvent.input(screen.getByTestId('layer-form-name'), { target: { value: 'base' } });
    expect(screen.getByTestId('layer-form-submit')).toBeDisabled();
  });
  it('submits on Enter in the name field, but not when invalid', async () => {
    const onsubmit = vi.fn();
    render(LayerChangeForm, { mode: 'create', catalogs: ['personal'], catalog: 'personal', onsubmit, oncancel: vi.fn() });
    const name = screen.getByTestId('layer-form-name');
    await fireEvent.input(name, { target: { value: 'Bad' } });
    await fireEvent.submit(screen.getByTestId('layer-form'));
    expect(onsubmit).not.toHaveBeenCalled();
    await fireEvent.input(name, { target: { value: 'good' } });
    await fireEvent.submit(screen.getByTestId('layer-form'));
    expect(onsubmit).toHaveBeenCalledTimes(1);
  });
});
