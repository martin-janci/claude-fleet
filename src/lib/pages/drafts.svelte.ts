// A settings page's unsaved typed values (Orbit Fleet G1.5, FormsAnatomy
// "Settings field row"): a number, a duration, a choice or a line of text
// is staged here as the person types and written only when they press Save
// on the page's bar ("2 changes · Discard · Save"). Switches and other
// toggles never come here: they save at once and offer Undo.
import type { Result } from '../result';

export type DraftWriter = (key: string, value: string) => Promise<Result<unknown>>;

export class SettingDrafts {
  /** key → the value the person typed, for keys whose stored value differs. */
  staged = $state<Record<string, string>>({});
  /** key → why the last Save could not write it. The value stays staged. */
  errors = $state<Record<string, string>>({});
  saving = $state(false);

  get count(): number {
    return Object.keys(this.staged).length;
  }

  /** The value a row shows: the staged one, else the stored one. */
  valueOf(key: string, stored: string): string {
    return Object.hasOwn(this.staged, key) ? this.staged[key] : stored;
  }

  has(key: string): boolean {
    return Object.hasOwn(this.staged, key);
  }

  /** Stage `value` for `key`; typing the stored value back unstages it. */
  stage(key: string, value: string, stored: string): void {
    const { [key]: _was, ...rest } = this.staged;
    this.staged = value === stored ? rest : { ...rest, [key]: value };
    if (Object.hasOwn(this.errors, key)) {
      const { [key]: _err, ...others } = this.errors;
      this.errors = others;
    }
  }

  discard(): void {
    this.staged = {};
    this.errors = {};
  }

  /** Write every staged value, in the order it was staged. A written one
   *  leaves the draft; a refused one stays, with the refusal beside its
   *  row. Resolves to how many were refused. */
  async save(write: DraftWriter): Promise<number> {
    if (this.saving) return 0;
    this.saving = true;
    const errors: Record<string, string> = {};
    try {
      for (const [key, value] of Object.entries(this.staged)) {
        const r = await write(key, value);
        if (r.ok) {
          const { [key]: _done, ...rest } = this.staged;
          this.staged = rest;
        } else {
          errors[key] = r.error.message;
        }
      }
    } finally {
      this.errors = errors;
      this.saving = false;
    }
    return Object.keys(errors).length;
  }
}
