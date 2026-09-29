# ag — one launcher for coding-agent CLIs

`ag` starts Claude Code or Codex with one set of flags, gives you alias
commands (`cl`, `cx`, …) that work everywhere — tmux panes, `ssh host cmd`,
claude-fleet sessions — and tells you what is missing on a machine.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/martin-janci/claude-fleet/main/tools/ag/install.sh | bash
```

Then `ag doctor`. claude-fleet installs `ag` on every host it provisions.

## Use

```bash
ag                     # default agent
ag codex               # a specific one
ag -p "explain this"   # one-shot, prints the answer
ag -c                  # continue the last session
ag -r <id>             # resume a session
ag --yolo              # skip permission prompts (opt-in)
ag claude -- --foo     # anything after -- goes to the CLI unchanged
```

| ag flag | claude | codex |
|---|---|---|
| `-p PROMPT` | `-p PROMPT` | `exec … PROMPT` |
| `-c` | `--continue` | `resume --last` |
| `-r ID` | `--resume ID` | `resume ID` |
| `--new-id ID` | `--session-id ID` | unsupported (exit 3) |
| `--name N` | `--name N` | ignored |
| `-m M` | `--model M` | `-m M` |
| `--effort E` | `--effort E` | `-c model_reasoning_effort="E"` |
| `--yolo` | `--dangerously-skip-permissions` | `--dangerously-bypass-approvals-and-sandbox` |

## Config — `~/.config/ag/config`

```ini
default = claude
order = claude codex
yolo = false

[alias]
cl = claude --yolo
cx = codex
```

Run `ag shims` after editing `[alias]`. Alias values are plain words.

## Exit codes

`2` usage · `3` flag unsupported by the harness · `4` harness not installed · `5` config / filesystem.

## Adding a harness

Create `tools/ag/drivers/<name>.sh` defining `drv_bin`, `drv_install_hint`
and `drv_argv` (fill the `ARGV` array from the `AG_*` globals in
`lib/args.sh`; return 3 for a flag the CLI cannot honour), and add its cases to
`scripts/ag-test.sh`. Keep it bash 3.2 compatible.
