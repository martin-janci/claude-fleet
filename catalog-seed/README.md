# catalog-seed

Assets claude-fleet ships for your asset catalog (see
`docs/superpowers/specs/2026-09-14-asset-catalog-design.md`). This directory
is a valid catalog root, so it can be loaded as-is in a test; to use an
asset, copy its folder into your catalog's checkout, commit, and reload
(`fleet-hub catalog reload`, or Assets → Reload on the desktop). The next
sync installs it on your hosts.

| Asset | What it is for |
|---|---|
| `skills/fleet-guides` | Lets a Claude session on a host write a step-by-step guide for Settings → Guides and propose it through the `claude-fleet` MCP `guide` tool; a person approves it (`docs/pages.md` → *Guides*). |

`service::guides` tests load this catalog with the catalog's own reader,
lint it, and render the skill as sync installs it.
