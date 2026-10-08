# BottomSheet

The phone's dialog: filters and grouping, Move, To review, ticket, a `ChatForm` from Control. It rises over a scrim with a grip and a title; actions sit at the bottom, in thumb reach.

**The consumer provides** the title, the content, and the footer actions (Cancel plus one primary).

**Do**
- Put the primary action last and full height (`touch-min`); when a choice is missing, the button names it ("Choose a host") and stays disabled.
- Keep choices unselected unless the person chose before. A Jev proposal marks one option with `of-ai-pre` and the ✦ line; host choice by numbers never gets one.
- Use `radius-sheet` on the top corners and `phone-gutter` on the sides.

**Don't**
- Don't use a sheet for a result: a repair report or a summary lands in the conversation or the card it belongs to.
- Don't put a bulk action above the items it acts on.
