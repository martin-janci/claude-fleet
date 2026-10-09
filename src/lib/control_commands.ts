// Control's own slash commands (Orbit Fleet redesign step 9.6). Claude Code
// runs them from the operator's directory, where fleet writes them as
// `.claude/commands/<name>.md` (`CONTROL_COMMANDS` in
// crates/fleet-core/src/service/operator.rs, which a Rust test holds this
// list to). The operator's directory is not a repository, so the menu cannot
// read them the way it reads a project's: it lists these for the operator's
// session and for no other.
import type { SlashCommand } from './conversation';

export const CONTROL_COMMANDS: readonly SlashCommand[] = [
  { name: 'task', description: 'Create a task', args: true, source: 'command' },
  { name: 'plan', description: 'Plan subtasks for a task or goal', args: true, source: 'command' },
  { name: 'done', description: 'Mark a task done', args: true, source: 'command' },
  { name: 'assign', description: 'Assign a task to a session', args: true, source: 'command' },
  { name: 'start', description: 'Start a session on a task', args: true, source: 'command' },
];
