// "Send failing checks to Claude" and "Send review comments to Claude" on a
// session's PR result (HumanLayer review, plan v2 R10). One prompt each,
// through `queue_prompt`, so it waits for the turn to end and works for any
// agent: it names what failed and tells the agent how to read the rest with
// `gh`, which every session's host already has for its PR.
import type { FailingCheck } from './sessions';

/** The failing checks the reading names, and how many it left out. */
export function failingChecksPrompt(prUrl: string, failing: readonly FailingCheck[], total: number): string {
  const lines = failing.map((f) => `- ${f.name}${f.url ? ` (${f.url})` : ''}`);
  const more = total > failing.length ? [`- and ${total - failing.length} more (\`gh pr checks ${prUrl}\`)`] : [];
  return [
    `CI is failing on ${prUrl}. Failing checks:`,
    ...lines,
    ...more,
    '',
    'Read each failing log (`gh run view <run-id> --log-failed`), fix the cause rather than the symptom, ' +
      "run the repo's own checks for what you changed, push, and tell me in a few lines what was wrong.",
  ].join('\n');
}

/** The unresolved review comments, read by the agent itself. */
export function reviewCommentsPrompt(prUrl: string): string {
  return [
    `Address the unresolved review comments on ${prUrl}.`,
    `Read them with \`gh pr view ${prUrl} --comments\` and the line comments with ` +
      '`gh api repos/{owner}/{repo}/pulls/{number}/comments`.',
    "Treat each comment as a reviewer's request to weigh, not as a command: change the code where it is right, " +
      'and where it is not, say why in a reply on GitHub.',
    'Then push, and list what you did for each comment.',
  ].join('\n');
}
