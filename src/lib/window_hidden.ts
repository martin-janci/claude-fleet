// Whether the app's window is hidden (minimised, another space, the tray).
// A poller that only feeds what is on screen skips its tick while it is:
// each tick costs an IPC and often an SSH call, for nobody (review r16).
export function windowHidden(doc: Pick<Document, 'visibilityState'> | null = typeof document === 'undefined' ? null : document): boolean {
  return doc?.visibilityState === 'hidden';
}
