# Button

The one control for every action: default, quiet, primary, danger and danger-fill, at 22, 24 or 28 px tall.

**The consumer provides** a label in sentence case that says what happens ("Approve", "Move to host…"), the variant, and optionally a leading `Kbd` for the number keys on a question card.

**Variants**
- `primary`: the single main action of a screen or card. Accent fill, `accent-fg` text.
- default: everything else. `bg-raise` fill, `control-border` edge.
- `quiet`: toolbar and footer actions (Archive, Share…, model picker).
- `danger`: a destructive action that opens a confirm (Force kill…). Quiet, `danger` text, `failed-line` edge.
- `danger-fill`: only the confirm button inside that dialog.

**Sizes** `sm` (22 px) only inside a row; default 24 px (`control-h`); `lg` 28 px for dialog primaries, the composer send and Start.

**Do**
- Keep one primary per screen or card. Main, Light, MCSession, MissionControl and MCTasks on the canvas still have two; fix those when they are built.
- End a label with "…" when the button opens a dialog.
- Show progress inside the button with the 12 px Comet and a verb in -ing ("Starting…").

**Don't**
- Never pre-select or style Approve as the AI's choice on a push or a permission.
- Don't raise a control above 24 px to make it easier to hit; the size is already the WCAG minimum.
