# BottomBar

The phone's five destinations: **Inbox, Sessions, Control, Work, More**. Inbox opens first.

**The consumer provides** the current destination (`aria-current="page"`) and the Needs you count for Inbox.

**Do**
- Show one badge only, on Inbox: the Needs you count, the same number as the list, the notification and the desktop.
- Keep every target at `touch-min` (48 px) or more; the bar is `tab-bar-h` (72 px).
- Put Hosts, Files, Accounts, Organisations and Settings under More, each with a live line ("1 signal lost").

**Don't**
- Don't add a second badge or a red dot on More; what needs you goes to Inbox.
- Don't stack FABs over the bar. Sessions has one "New session" button; Control replaces the agent button.
