// Control's own slash commands (Orbit Fleet redesign step 9.6). Claude Code
// runs them from the operator's directory, where fleet writes them as
// `.claude/commands/<name>.md` (`CONTROL_COMMANDS` in
// crates/fleet-core/src/service/operator.rs, which a Rust test holds this
// list to). The operator's directory is not a repository, so the menu cannot
// read them the way it reads a project's: it lists these for the operator's
// session and for no other.
//
// Gap plan G3.9: the desktop runs /task, /done, /assign and /start itself
// (`control_slash.ts`) and only /plan reaches the agent; `usage` is the
// grammar `parseControlCommand` reads, shown in the menu.
import type { SlashCommand } from './conversation';

export const CONTROL_COMMANDS: readonly SlashCommand[] = [
  { name: 'task', description: 'Create a task', args: true, usage: '<title> due:<day> @owner', source: 'command' },
  { name: 'plan', description: 'Plan subtasks for a task or goal', args: true, usage: '#KEY or a goal', source: 'command' },
  { name: 'done', description: 'Mark a task done', args: true, usage: '#KEY', source: 'command' },
  { name: 'assign', description: 'Assign a task to a session', args: true, usage: '#KEY @session', source: 'command' },
  { name: 'start', description: 'Start a session on a task', args: true, usage: '#KEY @host', source: 'command' },
];
