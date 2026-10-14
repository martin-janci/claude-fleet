// The app's wizards an agent's `wizard` block may open in the chat (redesign
// 10.12; `CHAT_WIZARDS` in crates/fleet-core/src/pages/chat_blocks.rs, the
// same list in the same order). Each has a spec in `wizards/<id>.json` and a
// last button that runs in the chat (`chat_wizard_runs.ts`). `link_hub`
// stays in Settings › Hub. Kept apart from wizards.ts so the block check
// does not pull every spec in.
// `fleet_agent` has no spec file: its form is a catalog fleet agent's grant
// form, built by the hub when the card opens (`definition` names it).
export const CHAT_WIZARD_IDS = ['add_host', 'add_project', 'fleet_agent', 'get_started', 'new_session', 'pair_device'] as const;
export type ChatWizardId = (typeof CHAT_WIZARD_IDS)[number];
