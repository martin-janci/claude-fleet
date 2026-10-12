// "Discard changes?" (Orbit Fleet step G1.2): closing a changed form asks
// once, inline in its footer. The first close of a changed form asks; a
// second close while the question is up (Escape again, Discard) closes.
// Keep editing puts the footer back.
export class CloseGuard {
  asking = $state(false);
  #dirty: () => boolean;
  #close: () => void;

  constructor(dirty: () => boolean, close: () => void) {
    this.#dirty = dirty;
    this.#close = close;
  }

  /** Cancel, Escape or the backdrop. */
  request = (): void => {
    if (this.asking || !this.#dirty()) {
      this.asking = false;
      this.#close();
      return;
    }
    this.asking = true;
  };

  keep = (): void => {
    this.asking = false;
  };

  discard = (): void => {
    this.asking = false;
    this.#close();
  };
}
