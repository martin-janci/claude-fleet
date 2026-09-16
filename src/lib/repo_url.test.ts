import { describe, it, expect } from 'vitest';
import { parseRepoUrl, cloneUrlFor, isComponent } from './repo_url';

// Ported verbatim from `src-tauri/src/repo_url.rs`'s `#[cfg(test)] mod tests`.
// Any new Rust test case needs the identical case added here.

describe('parseRepoUrl', () => {
  it('parses every accepted github form', () => {
    for (const input of [
      'martin-janci/claude-fleet',
      'https://github.com/martin-janci/claude-fleet',
      'https://github.com/martin-janci/claude-fleet.git',
      'http://github.com/martin-janci/claude-fleet',
      'https://github.com/martin-janci/claude-fleet/',
      'git@github.com:martin-janci/claude-fleet.git',
      'git@github.com:martin-janci/claude-fleet',
      'ssh://git@github.com/martin-janci/claude-fleet.git',
      '  martin-janci/claude-fleet  ',
    ]) {
      expect(parseRepoUrl(input), input).toEqual({
        owner: 'martin-janci',
        repo: 'claude-fleet',
      });
    }
  });

  it('rejects what is not a github repo', () => {
    for (const bad of [
      '',
      '   ',
      'claude-fleet',
      'martin-janci/',
      '/claude-fleet',
      'martin-janci/claude-fleet/extra',
      'https://gitlab.com/o/r',
      'https://github.com/',
      'https://github.com/only-owner',
      '../etc/passwd',
      'martin-janci/../escape',
      'o/r; rm -rf /',
      // A `.git` component would make the parent directory resolve as
      // a git repo (`<path>/.git` is this codebase's project marker).
      'owner/.git.git',
      '.git/repo',
      'git@github.com:o/.git.git',
      // All-dots components, beyond plain `.`/`..`.
      '.../repo',
      'owner/...',
      // A leading `-` would be read as a command-line option by a
      // program the pair is later passed to, not a name.
      '-owner/repo',
      'owner/-repo',
      // GitHub's length caps: 39 for owner, 100 for repo.
      `${'a'.repeat(40)}/repo`,
      `owner/${'a'.repeat(101)}`,
      // Non-ASCII, control characters, and percent-encoded traversal.
      'о/repo', // Cyrillic о (U+043E), not ASCII 'o'
      'owner/re\0po',
      'owner/re\npo',
      '%2e%2e/repo',
      // URL edge cases that must not be mistaken for github.com.
      'https://github.com/o/r?x=1',
      'https://user:pw@github.com/o/r',
      'https://github.com.evil.com/o/r',
      'git@github.com:/o/r',
      'https://github.com:443/o/r',
    ]) {
      expect(parseRepoUrl(bad), bad).toBeNull();
    }
  });

  it('accepts legitimate dotted components', () => {
    // A leading dot alone is fine — only all-dots and the exact `.git`
    // component are rejected.
    expect(parseRepoUrl('owner/.github')).toEqual({ owner: 'owner', repo: '.github' });
    expect(parseRepoUrl('.hidden/repo')).toEqual({ owner: '.hidden', repo: 'repo' });
    expect(parseRepoUrl('owner/repo.name')).toEqual({ owner: 'owner', repo: 'repo.name' });
  });

  it('clone url is always github ssh', () => {
    expect(cloneUrlFor('martin-janci', 'claude-fleet')).toBe(
      'git@github.com:martin-janci/claude-fleet.git',
    );
  });

  it('clone url round-trips through parse', () => {
    for (const [owner, repo] of [
      ['martin-janci', 'claude-fleet'],
      ['owner', '.github'],
      ['owner', 'repo.name'],
      ['a', 'b'],
    ]) {
      expect(parseRepoUrl(cloneUrlFor(owner, repo)), `${owner}/${repo}`).toEqual({ owner, repo });
    }
  });

  it('is_component rejects a leading dash', () => {
    // `service::add_project`'s adopt fallback reuses this directly on a
    // raw filesystem basename (e.g. `-rf`), which a command-line parser
    // would otherwise read as an option rather than a name.
    expect(isComponent('-rf', 100)).toBe(false);
    expect(isComponent('-', 100)).toBe(false);
    expect(isComponent('rf-', 100)).toBe(true);
    expect(isComponent('r-f', 100)).toBe(true);
  });
});
