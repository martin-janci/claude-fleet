# StatusChip

A one-word state label tinted from its status token, plus the neutral chip for hosts, PRs and filters, and the 8 px status dot.

**The consumer provides** a state: `waiting`, `working`, `failed`, `done` or `idle`, or no state for a neutral chip. Map domain states onto these: a question, permission, grant to sign or push to approve is `waiting` and reads **Needs you**; stuck, CI red and blocked are `failed`; completed, CI green and merged are `done`; paused, stopped and queued are `idle`.

**Do**
- Keep the word; colour alone never carries the state. The dot always has an `aria-label`.
- Use the `accent` chip only for things Claude did for you ("Sent to a session").

**Don't**
- Don't put more than one status chip on a row.
- Don't use amber for anything that does not need a person.
