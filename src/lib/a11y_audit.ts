// The accessibility audit component tests run (Orbit Fleet redesign step 7.2).
// A small in-repo checker over a rendered DOM, in the spirit of axe's rules
// for what this app can get wrong: a control nobody can name, a tab with no
// selected state, a popup trigger with no expanded state, a duplicate id, an
// image with no text, a selected row that only a colour says is selected, a
// ▸/▾ disclosure with no expanded state, a status dot with no word. It reads the DOM only; jsdom has no layout, so target
// size and the 11px floor are checked on the source by `a11y_lint.test.ts`.

export interface A11yViolation {
  rule:
    | 'name'
    | 'tab-selected'
    | 'popup-expanded'
    | 'disclosure-expanded'
    | 'selected-state'
    | 'status-word'
    | 'duplicate-id'
    | 'img-alt'
    | 'label-for';
  /** A short pointer at the element: tag, testid or class, and its text. */
  where: string;
}

const CONTROLS = [
  'button',
  'a[href]',
  'select',
  'textarea',
  'input:not([type="hidden"])',
  '[role="button"]',
  '[role="tab"]',
  '[role="menuitem"]',
  '[role="menuitemradio"]',
  '[role="menuitemcheckbox"]',
  '[role="checkbox"]',
  '[role="radio"]',
  '[role="switch"]',
  '[role="option"]',
  '[role="combobox"]',
  '[role="slider"]',
].join(', ');

function describe(el: Element): string {
  const id = el.getAttribute('data-testid');
  const cls = typeof el.className === 'string' ? el.className.split(/\s+/).filter((c) => c && !c.startsWith('svelte-'))[0] : '';
  const text = (el.textContent ?? '').trim().slice(0, 30);
  return `<${el.tagName.toLowerCase()}${id ? ` data-testid="${id}"` : cls ? ` .${cls}` : ''}>${text}`;
}

/** The element's accessible name, by the parts of the name computation that
 *  matter here: aria-labelledby, aria-label, a <label>, a wrapping <label>,
 *  text content (images by alt), placeholder, then title. */
export function accessibleName(el: Element): string {
  const doc = el.ownerDocument;
  const by = el.getAttribute('aria-labelledby');
  if (by) {
    const t = by
      .split(/\s+/)
      .map((id) => doc.getElementById(id)?.textContent?.trim() ?? '')
      .join(' ')
      .trim();
    if (t) return t;
  }
  const aria = el.getAttribute('aria-label')?.trim();
  if (aria) return aria;
  if (el.id) {
    const label = doc.querySelector(`label[for="${CSS.escape(el.id)}"]`);
    if (label?.textContent?.trim()) return label.textContent.trim();
  }
  const wrap = el.closest('label');
  if (wrap && wrap !== el && wrap.textContent?.trim()) return wrap.textContent.trim();
  const tag = el.tagName.toLowerCase();
  if (tag !== 'input' && tag !== 'select' && tag !== 'textarea') {
    const text = visibleText(el);
    if (text) return text;
  }
  if (tag === 'input' && ['button', 'submit', 'reset'].includes((el as HTMLInputElement).type)) {
    const v = (el as HTMLInputElement).value?.trim();
    if (v) return v;
  }
  const ph = el.getAttribute('placeholder')?.trim();
  if (ph) return ph;
  return el.getAttribute('title')?.trim() ?? '';
}

function visibleText(el: Element): string {
  let out = '';
  for (const n of Array.from(el.childNodes)) {
    if (n.nodeType === 3) out += n.textContent ?? '';
    else if (n.nodeType === 1) {
      const c = n as Element;
      if (c.getAttribute('aria-hidden') === 'true') continue;
      if (c.tagName.toLowerCase() === 'img') out += c.getAttribute('alt') ?? '';
      else if (c.getAttribute('aria-label')) out += c.getAttribute('aria-label');
      else out += visibleText(c);
    }
  }
  return out.trim();
}

/** Every violation in `root`. */
export function a11yAudit(root: ParentNode): A11yViolation[] {
  const out: A11yViolation[] = [];
  for (const el of Array.from(root.querySelectorAll(CONTROLS))) {
    if (el.closest('[aria-hidden="true"], [hidden], [inert]')) continue;
    if (!accessibleName(el)) out.push({ rule: 'name', where: describe(el) });
  }
  for (const el of Array.from(root.querySelectorAll('[role="tab"]'))) {
    if (!el.hasAttribute('aria-selected')) out.push({ rule: 'tab-selected', where: describe(el) });
  }
  for (const el of Array.from(root.querySelectorAll('[aria-haspopup]:not([aria-haspopup="false"])'))) {
    if (!el.hasAttribute('aria-expanded')) out.push({ rule: 'popup-expanded', where: describe(el) });
  }
  const seen = new Set<string>();
  for (const el of Array.from(root.querySelectorAll('[id]'))) {
    if (seen.has(el.id)) out.push({ rule: 'duplicate-id', where: describe(el) });
    seen.add(el.id);
  }
  for (const el of Array.from(root.querySelectorAll('img'))) {
    if (!el.hasAttribute('alt') && el.getAttribute('aria-hidden') !== 'true') out.push({ rule: 'img-alt', where: describe(el) });
  }
  // A ▸/▾ chevron says "this opens and closes": it must say which it is.
  for (const el of Array.from(root.querySelectorAll('button, [role="button"]'))) {
    const t = (el.textContent ?? '').trim();
    if (/^[▸▾▴▶▼▲]/.test(t) && !el.hasAttribute('aria-expanded') && !el.closest('[aria-expanded]'))
      out.push({ rule: 'disclosure-expanded', where: describe(el) });
  }
  // A selected row or control says so to a screen reader, not only in colour.
  const STATE = ['aria-current', 'aria-selected', 'aria-checked', 'aria-pressed'];
  for (const el of Array.from(root.querySelectorAll('.selected, .is-active'))) {
    const interactive = el.matches(CONTROLS) ? el : el.querySelector(CONTROLS);
    if (!interactive) continue;
    const carriers = [el, interactive];
    if (!carriers.some((c) => STATE.some((a) => c.hasAttribute(a))))
      out.push({ rule: 'selected-state', where: describe(el) });
  }
  // A status dot carries the colour; a word must carry the meaning: its own
  // label, or a status word beside it.
  for (const el of Array.from(root.querySelectorAll('.status-dot'))) {
    if (el.getAttribute('aria-label')?.trim()) continue;
    if (el.parentElement?.querySelector('.status-word')) continue;
    out.push({ rule: 'status-word', where: describe(el) });
  }
  const doc = (root as Node).ownerDocument ?? (root as Document);
  const LABELABLE = 'input, select, textarea, button, meter, output, progress';
  for (const el of Array.from(root.querySelectorAll('label[for]'))) {
    const target = doc.getElementById(el.getAttribute('for') ?? '');
    if (!target || !target.matches(LABELABLE)) out.push({ rule: 'label-for', where: describe(el) });
  }
  return out;
}

/** The violations as one line each, for an assertion message. */
export function a11yReport(root: ParentNode): string[] {
  return a11yAudit(root).map((v) => `${v.rule}: ${v.where}`);
}
