import { render } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import HostStrip from './HostStrip.svelte';

describe('HostStrip', () => {
  it('renders one dot per host in order, in that order', () => {
    const { container } = render(HostStrip, { order: ['local', 'htz', 'trn'], present: ['local', 'trn'] });
    const dots = container.querySelectorAll('.dot');
    expect(dots).toHaveLength(3);
  });

  it('classifies each host as present, differs, or absent, and reflects that in the aria-label', () => {
    const { container } = render(HostStrip, {
      order: ['local', 'oci', 'trn'],
      present: ['local', 'oci'],
      odd: ['oci'],
    });
    const dots = container.querySelectorAll('.dot');
    expect(dots[0].className).toContain('present');
    expect(dots[0].className).not.toContain('differs');
    expect(dots[1].className).toContain('differs');
    expect(dots[2].className).toContain('absent');

    const strip = container.querySelector('.strip');
    expect(strip?.getAttribute('aria-label')).toBe('local: present, oci: differs, trn: absent');
  });

  it('a host in odd but not present still reads as absent, not differs', () => {
    const { container } = render(HostStrip, { order: ['local'], present: [], odd: ['local'] });
    const dot = container.querySelector('.dot');
    expect(dot?.className).toContain('absent');
    expect(dot?.className).not.toContain('differs');
  });
});
