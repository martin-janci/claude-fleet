# ChatForm

A form or wizard that Control or an agent writes into a conversation as a `fleet.form/1` spec; the person fills it in place and the answers go back to whoever asked.

**The consumer provides** the spec (title, optional intro, 1 to 12 steps of fields, the submit label), who asked and why ("why" is the agent's one sentence), and the state: building, open, sending, answered, declined, cancelled or expired. Field types are `text`, `textarea`, `number`, `bool` (a `Toggle`), `select` (radio options), `multiselect` (checkbox options) and `secret`.

**States**
- **Building:** while Control is still writing the spec, the card shows its title, step bars and skeleton fields that fill in as they arrive, and a small Atom with what Control is reading. The person can type in the first step as soon as it exists.
- **Open:** step chrome (Step 2 of 4, Back and Next, step bars) appears from two visible steps on; a one-step form is a plain card with its submit button. An open form puts the session in Needs you.
- **Sending:** the Comet in the submit button with an -ing verb ("Creating…"); fields stay readable.
- **Answered / declined:** the card collapses to one line with who answered and a summary, or the decline note. The work it started shows next (a Pulse sequence for a new session).

**Do**
- Treat the form as the only way a generated wizard acts: nothing runs until the person presses the last step's button.
- Let Jev propose at most one choice per field, with the ✦ pill, a reason and Change; mark that field with `of-ai-pre`.
- Show a secret field as dots with where it is stored ("stays on mercury"); the value is written to the session's host and never reaches the agent or a log.
- Offer Decline… on every open form; the note goes back to the agent.

**Don't**
- Don't let Jev or an LLM pre-fill trust, shares, an organisation, or an Approve.
- Don't open a form as a modal or a full-screen overlay; it lives in the conversation, on the desktop, a paired desktop and the phone alike.
