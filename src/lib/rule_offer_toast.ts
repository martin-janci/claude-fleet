// The rule-suggestion toast (Toasts board): after a start that makes fleet
// offer a start rule (redesign 8.11: five identical starts of a key's
// prefix in one repository), the corner says "You picked acme/pos for PD-*
// 5 times" over "Add rule PD-* → acme/pos?", with Add rule and Not now.
//
// The offer is fleet's count, not a guess, and nothing changes until Add
// rule is pressed. Not now only closes the toast: the offer stays in
// Automation › Rules and on the next start popover of a matching task. One
// toast per offer per window.
import { acceptStartRule, listStartRules, ruleLine, ruleProject, type StartRuleView } from './start_rules';
import { push, pushError } from './toasts';

/** Mirrors `service::start_rules::glob_match`: `*` is any run of
 *  characters, everything else itself, ASCII case ignored. */
export function ruleMatches(pattern: string, key: string): boolean {
  const re = pattern
    .split('*')
    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('.*');
  return new RegExp(`^${re}$`, 'i').test(key);
}

/** The offer to toast for a start of `key`, or `null`: a rule fleet offers
 *  for it that this person may answer and this window has not toasted. */
export function offerFor(rules: readonly StartRuleView[], key: string, shown: ReadonlySet<number>): StartRuleView | null {
  return (
    rules.find((r) => r.state === 'offered' && r.may_change !== false && !shown.has(r.id) && ruleMatches(r.pattern, key)) ??
    null
  );
}

/** The toast's two lines for `offer`. */
export function ruleOfferLines(offer: StartRuleView): { message: string; sub: string } {
  const n = offer.confirmations ?? 5;
  return {
    message: `You picked ${ruleProject(offer)} for ${offer.pattern} ${n} ${n === 1 ? 'time' : 'times'}`,
    sub: `Add rule ${ruleLine(offer)}?`,
  };
}

const shown = new Set<number>();

/** After a start of `key`: toast the rule fleet now offers for it, if any.
 *  Best effort; a failed read says nothing. Resolves to the toast's id. */
export async function offerRuleAfterStart(key: string | null | undefined): Promise<number | null> {
  if (!key) return null;
  const r = await listStartRules();
  if (!r.ok || !Array.isArray(r.value)) return null;
  const offer = offerFor(r.value, key, shown);
  if (!offer) return null;
  shown.add(offer.id);
  const { message, sub } = ruleOfferLines(offer);
  return push({
    kind: 'info',
    message,
    sub,
    action: {
      label: 'Add rule',
      run: () =>
        void acceptStartRule(offer.id).then((a) => {
          if (!a.ok) pushError(a.error, 'Add rule failed');
          else push({ kind: 'success', message: `Rule added: ${ruleLine(a.value)}`, sub: 'Edit it in Automation › Rules' });
        }),
    },
    secondary: { label: 'Not now', run: () => {} },
  });
}

/** Tests only. */
export function _resetRuleOfferToastsForTests(): void {
  shown.clear();
}
