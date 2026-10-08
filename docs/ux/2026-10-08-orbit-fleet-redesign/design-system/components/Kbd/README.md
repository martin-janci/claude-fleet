# Kbd

A small outlined key hint shown beside the action it triggers.

**The consumer provides** the Mac chord. On Windows and Linux render the platform chord instead (⌘ becomes Ctrl, ⌥⌘ becomes Ctrl+Alt) and put the other platform's chord in the `title`.

**Do**
- Put it at the end of a button or tab label, or inside a primary button for the 1/2/3 answers of a question card.
- Use the chords in the Keyboard section; every 0.5.3 shortcut keeps working.

**Don't**
- Don't show a hint for a chord that does not exist on that screen (⌥⌘P only on the Files tab).
