# Keyboard

Every 0.5.3 shortcut keeps working, except ⌘E, which now opens Control. The registry, `src/lib/shortcuts.ts`, is the one source; this page follows it, and `src/lib/keyboard_doc.test.ts` fails when a global chord below no longer matches it.

Off the Mac the chords do not simply swap ⌘ for Ctrl. Plain Ctrl+K, Ctrl+J and the like belong to the program in the terminal, so the app's ⌘ chords (⌘K, ⌘P, ⌘N, ⌘J, ⌘E, ⌘I and the ⌘⇧ ones) become **Ctrl+Shift** chords, and ⌥⌘ becomes **Ctrl+Alt**. ⌘, is Ctrl+, and ⌘I is Ctrl+Shift+H. The Windows · Linux column is the chord to show there.

## Everywhere

| Mac | Windows · Linux | Action |
| --- | --- | --- |
| ⌘K, ⌘P | Ctrl+Shift+K, Ctrl+Shift+P | Quick switcher and commands |
| ⌘N | Ctrl+Shift+N | New session |
| ⌘J | Ctrl+Shift+J | Flip the Session view (agent tab) |
| ⌘I | Ctrl+Shift+H | Accounts and hosts |
| ⌘E | Ctrl+Shift+E | Control, the fleet agent |
| ⌘, | Ctrl+, | Settings |
| ⌘⇧W | Ctrl+Shift+W | Switch Sessions and Work |
| ⌘⇧O | Ctrl+Shift+O | Organisation scope |
| ⌘⇧T | Ctrl+Shift+T | Today |
| ⌘⇧E | Ctrl+Alt+E | Open in VS Code |
| ⌥⌘B | Ctrl+Alt+B | Inspector |
| ⌥⌘T | Ctrl+Alt+T | New terminal |
| ⌘` | Ctrl+Alt+` | Next terminal |
| ⌥⌘P | Ctrl+Alt+P | Go to file (Files tab only) |
| ⌥⌘N | Ctrl+Alt+N | Next session that needs you |
| ⌘1–⌘9 | — | Open the 1st–9th session in the list (Mac only: off the Mac Ctrl+digit and Alt+digit belong to the terminal and the window manager) |
| ? | ? | Keyboard shortcuts (not while a text box has focus) |

## In a view

| Mac | Windows · Linux | Action |
| --- | --- | --- |
| ⌘F | Ctrl+F | Find in the conversation |
| [ and ] | [ and ] | Previous and next turn in the conversation |
| Enter | Enter | Send, in the message box |
| ⇧Enter, ⌥Enter | Shift+Enter, Alt+Enter | New line, in the message box |
| ↑ and ↓ | ↑ and ↓ | Earlier and later prompts, from an empty message box |
| Tab, Esc | Tab, Esc | Complete or close the slash menu and the task suggestion |
| 1, 2, 3 | 1, 2, 3 | Answer a question card |

## Forms and dialogs

⌘↵ (Ctrl+Enter) submits a form from any field. A bare Enter submits only a form with one text field; in a form with more, Enter in a field does nothing and Enter in a textarea is a new line. The primary verb shows the chord ("Create ⌘↵"); `formKeys` and `submitHint` in `src/lib/forms/form_frame.ts` give a dialog both.

Show a chord as a `Kbd` at the end of the control it triggers, from the registry (`shortcut=` or `shortcutLabel`), never as a literal ⌘ in markup, and the other platform's chord in its `title`.
