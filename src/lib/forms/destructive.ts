// The destructive confirm (Orbit Fleet step G1.4, the FormsAnatomy board,
// "Destructive confirm": delete, remove). A red verb, a safer way out on
// the left ("Pause it instead"), and a typed name only when the loss is
// large. `DestructiveConfirm.svelte` draws it; the rules live here.

/** From this many things lost (runs, rules, hosts, …) the person types the
 *  name to confirm. Below it, the red verb alone is enough. */
export const LARGE_LOSS = 5;

/** Whether `loss` things going is large enough to ask for the typed name. */
export function needsTypedName(loss: number): boolean {
  return Number.isFinite(loss) && loss >= LARGE_LOSS;
}

/** Whether `typed` names `name`: exact, around spaces trimmed. An empty name
 *  never matches (there is nothing to type). */
export function typedNameMatches(typed: string, name: string): boolean {
  const want = name.trim();
  return want.length > 0 && typed.trim() === want;
}

/** Why the verb is off while the typed name does not match yet. */
export function typedNameWhy(noun: string): string {
  return `Type the ${noun} name to confirm.`;
}

/** "41 runs", "1 rule": a count and its noun, for the loss sentence. */
export function countOf(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
