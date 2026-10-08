# Meter

A 4 px bar for context, quota and spend, always next to the number it shows.

**The consumer provides** the fraction, a text label with the real numbers, and a level: ok (`status-done`), `warn` (`status-waiting`) or `crit` (`status-failed`).

**Don't**
- Don't show a meter without its number; the bar alone is decoration.
