// claude-fleet's Claude Code plugin: when Claude Code shows its next-prompt
// suggestion (the dim text in the empty prompt box), post it to fleet's
// `/hook` endpoint as a `PromptSuggestion` event, so fleet's clients can
// offer it too. Claude Code has no settings hook for suggestions; this is
// the only way they leave the terminal.
//
// The endpoint and its bearer come from fleet's own http hooks in
// `~/.claude/settings.json` (the entry carrying `X-Fleet-Pane`), so this file
// holds no host-specific value. Without them, or outside tmux, it does nothing.
import type { Register } from 'claude-code'

type FleetHook = { url: string; authorization: string }

type SettingsHook = {
  type?: unknown
  url?: unknown
  headers?: Record<string, unknown>
}

function fleetHookIn(settings: unknown): FleetHook | undefined {
  const events = (settings as { hooks?: Record<string, unknown> } | null)?.hooks
  if (!events || typeof events !== 'object') return undefined
  for (const groups of Object.values(events)) {
    if (!Array.isArray(groups)) continue
    for (const group of groups) {
      const hooks = (group as { hooks?: unknown } | null)?.hooks
      if (!Array.isArray(hooks)) continue
      for (const h of hooks as SettingsHook[]) {
        const authorization = h?.headers?.Authorization
        if (
          h?.type === 'http' &&
          typeof h.url === 'string' &&
          h.url.endsWith('/hook') &&
          typeof authorization === 'string' &&
          h.headers?.['X-Fleet-Pane'] !== undefined
        ) {
          return { url: h.url, authorization }
        }
      }
    }
  }
  return undefined
}

export const register: Register = (on) => {
  on('prompt.suggest', async ($, e, next) => {
    const result = await next(e)
    if (e.origin.kind !== 'suggestion' || !result.isShown) return result
    try {
      const pane = await $.env.get('TMUX_PANE')
      const home = await $.env.get('HOME')
      if (!pane || !home) return result
      const settings = await $.fs.read(`${home}/.claude/settings.json`)
      const hook = fleetHookIn(JSON.parse(settings))
      if (!hook) return result
      await $.http.fetch(hook.url, {
        method: 'POST',
        headers: {
          Authorization: hook.authorization,
          'X-Fleet-Pane': pane,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          hook_event_name: 'PromptSuggestion',
          session_id: await $.session.id(),
          suggestion: e.text,
        }),
      })
    } catch {
      // Best-effort: fleet unreachable or settings unreadable never touches
      // the suggestion Claude Code already shows.
    }
    return result
  })
}
