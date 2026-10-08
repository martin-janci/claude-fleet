# KeyValue

A two-column list of facts for the inspector and detail panes: an 84 px muted label, then the value.

**The consumer provides** the label and value pairs. Values may hold a status dot, a link, mono text or a `Meter`.

**Do**
- Wrap long mono values with `overflow-wrap:anywhere`; never widen the pane.
- Use `tabular-nums` for money, counts and percentages.

**Don't**
- Don't put buttons in a value; actions go in their own row under the list.
