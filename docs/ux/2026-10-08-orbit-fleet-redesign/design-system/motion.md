# Motion and loaders

Twenty-four loaders, each with one job. The loader says what the app is really doing; if the app is waiting on a person, there is no loader, only text saying who and what.

## Rules

- A wait under `loader-delay` (400 ms) shows nothing.
- One loader per screen. Inside rows, buttons and the status bar only the Comet or the 16 px Orbit.
- Particle loaders sit on a dark stage in both themes; logo motions carry their own `brand-ink` tile.
- With reduced motion, or Motion set to Reduced or Off, every loop becomes a `loader-reduced` (2.4 s) opacity fade and Draw-on shows the finished mark.
- CSS and SVG only, under 100 nodes each, paused while the window or tab is hidden.
- Jev never shows a loader: its proposal appears when ready, or nothing does.
- In wizards and chats the loader sits inline where the answer will appear; never a full-screen overlay.

## Startup

A cold start takes about 1.6 s, and each stage shows the loader for what is really happening.

| When | What runs | Loader |
| --- | --- | --- |
| 0 ms | Window opens, store opens | Draw-on |
| 300 ms | Database migration, only if one runs | Progress ring, "Upgrading the database · 3 of 5" |
| 600 ms | Connecting to the hub (standalone skips) | Chase |
| 0.9 s | Finding hosts | Radar, one blip per host that answers |
| 1.3 s | Loading the fleet | Assemble, one particle per session: "22 sessions · 4 need you" |
| 1.6 s | Into the app | The mark shrinks into the header logo, Inbox fades in; a slow host keeps loading as a 16 px Orbit in the status bar |

- **Warm start** (opened again within 8 h): no splash; the last screen shows at once, Breathe in the status bar while it re-syncs.
- **First run**: Galaxy behind "Let's add your first host", then the Add host wizard with Radar.
- **Hub unreachable**: Signal lost after `hub-lost-after` (6 s) with Open offline, Retry and Hub settings.
- **After an update**: Wordmark reveal once, with "What's new".

## Where each loader goes

| Situation | Loader |
| --- | --- |
| Control plans a mission | Atom, then Constellation as work is sent to sessions |
| A mission is running | Comet trails on the mission card |
| Start a session (⌘N) | Pulse sequence: worktree, tmux, agent; the wizard closes into the new row |
| Move a session to another host | Particles stream from one host to the other |
| Add host wizard | Radar to discover, Sonar while each connection check runs |
| Pair a phone | Halo around the code while it waits for fleet-mobile to scan |
| Link two hubs | Counter-orbit during the key exchange, then Constellation; stops once it waits on a person |
| Search across hosts, ⌘K | Dot wave for hosts still answering; local results show at once |
| Download or update | Progress ring with the real size |
| Import assets | Data rain while the size is unknown |
| Tidy scans worktrees | Hex field |
| Control or an agent thinking | A small Atom with what it is reading |
| Sent to a session (chat) | A comet flies from the message to the session in the side panel |
| Agent tool calls | Comet only on the call that is running |
| Voice input | Sonar follows the mic level, then a Dot wave while it transcribes |
| A chat form is sent | Comet in the button; fields stay readable |
| Drafting with the LLM | Small Atom beside the Drafted field |
| Fork, rebase or merge | Liquid orbit |
| Long job toast | 28 px Progress ring |
| Hub lost | Gravity well banner, Signal lost after 6 s |
| Something needs you | Halo on the dock and tray icon |
| Tray and menu bar | Breathe, Chase, Halo, Signal lost |
| fleet-mobile pull to refresh | The Orbit draws as you pull, then Chase |

The canvas boards Startup, Loaders in use and Loaders in wizards and chats draw every case: https://claude.ai/artifact/B2sVtJEZodahNG4cvRu7Pu
