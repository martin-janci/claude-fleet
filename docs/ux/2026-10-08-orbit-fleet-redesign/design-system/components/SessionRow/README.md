# SessionRow

The row every session list uses: status dot, title and age on line one, what it waits on or did last on line two, then at most a few chips or inline actions.

**The consumer provides** the title (friendly name, else the work item), the status, one sentence for line two, an age, and optional chips (PR with CI result, host, Mission) or inline row actions (`Button` `sm`).

**Line two**, in order of preference: the pending question or permission ("Waiting for you: approve push to main"); a pause reason ("Paused · weekly limit on tech.silvester"); the latest activity; the last assistant sentence. Never the last prompt.

**Do**
- Mark the selected row with `aria-selected="true"`: `accent-soft` fill and a 2 px accent inset bar.
- Show a Jev proposal under line two with the `AISuggestion` pattern and a one-click undo ("Not waiting").
- Keep one height per row type; line two truncates.

**Don't**
- Don't let Jev or an LLM reorder Needs you; it sorts by time asked.
- Don't hide the summary behind a details toggle.
