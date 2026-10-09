// Transfer loaders (Orbit Fleet redesign step 10.10): which loader a
// transfer shows. A known size always gets the Progress ring with how far it
// is; an unknown one (an older hub that reports no bytes, an import that
// cannot say its total) gets Data rain, which promises nothing about when.

export type TransferLoader = { name: 'progress-ring'; value: number } | { name: 'data-rain' };

/** `fraction` is 0–1 when the size is known, null when it is not. */
export function transferLoader(fraction: number | null): TransferLoader {
  if (fraction === null || !Number.isFinite(fraction)) return { name: 'data-rain' };
  return { name: 'progress-ring', value: Math.min(1, Math.max(0, fraction)) };
}
