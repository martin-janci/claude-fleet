# Tabs

Underlined tabs for the views of one object, such as a session's Conversation, agent, Terminals, Files and Details.

**The consumer provides** the tab labels, the selected one (`aria-selected="true"`), and optional counts and shortcut hints.

**Do**
- Name the agent tab after the agent ("Claude Code" with its own icon; later Codex or Agy) so the app stays agent-agnostic.
- Keep plain shell terminals in their own Terminals tab, 0 to N per session.

**Don't**
- Don't use tabs to switch between different objects; that is the list.
