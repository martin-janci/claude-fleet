import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import ActiveFilters from './ActiveFilters.svelte';
import type { Facet } from './filter_facets';

const facet = (id: string): Facet => ({ id, label: `Label ${id}` }) as Facet;

/** The strip with an owner that drops a chip when it is cleared, as the
 *  filter bars do, and a Filters button to fall back to. */
function mountStrip(ids: string[]) {
  const fallback = document.createElement('button');
  fallback.textContent = 'Filters';
  document.body.appendChild(fallback);
  let facets = ids.map(facet);
  const r = render(ActiveFilters, {
    props: {
      facets,
      onclear: (id: string) => {
        facets = facets.filter((f) => f.id !== id);
        void r.rerender({ facets });
      },
      onclearall: () => {
        facets = [];
        void r.rerender({ facets });
      },
      emptyFocus: () => fallback,
    },
  });
  return { fallback, ...r };
}

describe('ActiveFilters', () => {
  it('announces the count, and still announces when the last filter is cleared', async () => {
    const { fallback } = mountStrip(['a']);
    const status = screen.getByTestId('active-filters-status');
    expect(status).toHaveAttribute('aria-live', 'polite');
    expect(status).toHaveTextContent('1 filter active');
    await fireEvent.click(screen.getByTestId('facet-a'));
    await waitFor(() => expect(screen.queryByTestId('active-filters')).toBeNull());
    // The same live region, still mounted, says so.
    expect(screen.getByTestId('active-filters-status')).toBe(status);
    expect(status).toHaveTextContent('No filters active');
    fallback.remove();
  });

  it('says nothing before any filter was on', () => {
    render(ActiveFilters, { props: { facets: [], onclear: () => {}, onclearall: () => {} } });
    expect(screen.getByTestId('active-filters-status').textContent).toBe('');
  });

  it('keeps focus in the strip when a chip is removed: the next chip, else the last, else the bar', async () => {
    const { fallback } = mountStrip(['a', 'b', 'c']);
    screen.getByTestId('facet-b').focus();
    await fireEvent.click(screen.getByTestId('facet-b'));
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('facet-c')));
    await fireEvent.click(screen.getByTestId('facet-c'));
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('facet-a')));
    await fireEvent.click(screen.getByTestId('facet-a'));
    await waitFor(() => expect(document.activeElement).toBe(fallback));
    fallback.remove();
  });

  it('Clear all hands focus to the bar', async () => {
    const { fallback } = mountStrip(['a', 'b']);
    screen.getByTestId('filters-clear-all').focus();
    await fireEvent.click(screen.getByTestId('filters-clear-all'));
    await waitFor(() => expect(document.activeElement).toBe(fallback));
    fallback.remove();
  });

  it('a click from outside the strip (focus elsewhere) does not steal focus', async () => {
    const { fallback } = mountStrip(['a', 'b']);
    const other = document.createElement('input');
    document.body.appendChild(other);
    other.focus();
    await fireEvent.click(screen.getByTestId('facet-a'));
    await waitFor(() => expect(screen.queryByTestId('facet-a')).toBeNull());
    expect(document.activeElement).toBe(other);
    other.remove();
    fallback.remove();
  });
});
