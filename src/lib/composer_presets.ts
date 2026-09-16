/**
 * Quick-action presets for the Conversation composer: a row of chips above
 * the prompt box. Per-app prefs (localStorage), editable in Settings.
 */
import { writable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { DEFAULT_REVIEW_PROMPT } from './sessions';

export interface ComposerPreset {
  label: string;
  text: string;
}

export const DEFAULT_PRESETS: ComposerPreset[] = [
  { label: 'Clear', text: '/clear' },
  { label: 'Compact', text: '/compact' },
  { label: 'Status', text: '/status' },
  { label: 'Continue', text: 'Continue where you left off.' },
  { label: 'Review', text: DEFAULT_REVIEW_PROMPT },
];

export function isPresetArray(v: unknown): v is ComposerPreset[] {
  return (
    Array.isArray(v) &&
    v.every(
      (p) =>
        typeof p === 'object' &&
        p !== null &&
        typeof (p as ComposerPreset).label === 'string' &&
        typeof (p as ComposerPreset).text === 'string',
    )
  );
}

export const composerPresets = writable<ComposerPreset[]>(
  readPref('composer-presets', DEFAULT_PRESETS, isPresetArray),
);
composerPresets.subscribe((v) => writePref('composer-presets', v));

export function resetComposerPresets(): void {
  composerPresets.set(DEFAULT_PRESETS.map((p) => ({ ...p })));
}

export function addPreset(): void {
  composerPresets.update((list) => [...list, { label: '', text: '' }]);
}

export function updatePreset(index: number, patch: Partial<ComposerPreset>): void {
  composerPresets.update((list) => list.map((p, i) => (i === index ? { ...p, ...patch } : p)));
}

export function removePreset(index: number): void {
  composerPresets.update((list) => list.filter((_, i) => i !== index));
}
