# Loader

The 24 Orbit loaders: 12 that animate the mark and 12 particle loaders, each with one job.

**The consumer provides** which wait it is (the job decides the loader, see the Motion section), a real count or size when there is one, and a line of text saying what is happening ("Upgrading the database · 3 of 5").

**Logo motion:** Orbit (default), Chase (connecting to the hub), Pulse sequence (session start, three real steps), Draw-on (splash), Progress ring (known size), Comet trails (long work), Breathe (idle and connected), Gravity well (reconnecting), Signal lost (offline, no spin), Wordmark reveal (after an update), Halo (needs you), Counter-orbit (two hubs syncing).

**Particle:** Particle swarm, Assemble (startup: one particle per session), Radar (finding hosts), Sonar (connection checks, voice), Dot wave (lists and search), Constellation (sync), Data rain (unknown size), Atom (agent or Control thinking), Galaxy (first run), Comet (buttons and rows, down to 12 px), Hex field (worktree and health scans), Liquid orbit (fork, rebase, merge).

**Do**
- Show a loader only after `loader-delay` (400 ms); one loader per screen.
- Inside rows, buttons and the status bar use only the Comet or the 16 px Orbit.
- Put particle loaders on a dark stage in both themes.
- Keep each under 100 nodes, CSS and SVG only, paused while hidden; with reduced motion every loop becomes a `loader-reduced` (2.4 s) opacity fade.

**Don't**
- Don't show a loader while waiting on a person (a grant, an approval, a scan); say who and what instead.
- Don't show a loader while Jev decides.
- No full-screen overlay in a wizard or chat; the loader sits where the answer will appear.
