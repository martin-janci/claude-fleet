# Banner

A full-width notice above a list or pane for a state that affects everything below it.

**The consumer provides** a state (`waiting` or `failed`), a bold one-line headline, one meta line with the evidence, and at most one action.

**Do**
- Lead with what happened, then what still works ("your sessions keep running on their hosts").
- Put a loader inside only when the app itself is working (Gravity well while reconnecting).

**Don't**
- Don't stack banners; the newest replaces the older one.
- Don't use a banner for one session's question; that is a QuestionCard.
