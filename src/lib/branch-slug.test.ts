import { describe, it, expect } from 'vitest';
import { slugifyBranch, finalizeBranchSlug, validateBranchName } from './branch-slug';

describe('slugifyBranch', () => {
  it('converts a natural sentence into a kebab slug', () => {
    expect(slugifyBranch('Fix the login bug')).toBe('fix-the-login-bug');
  });

  it('lowercases and strips punctuation', () => {
    expect(slugifyBranch('Feat: add NEW thing!')).toBe('feat-add-new-thing');
  });

  it('keeps a trailing dash so live typing stays responsive', () => {
    expect(slugifyBranch('fix login ')).toBe('fix-login-');
  });

  it('collapses runs of whitespace and underscores into a single dash', () => {
    expect(slugifyBranch('a   b__c')).toBe('a-b-c');
  });

  it('strips diacritics', () => {
    expect(slugifyBranch('účet píše')).toBe('ucet-pise');
  });

  it('drops leading dashes and dots', () => {
    expect(slugifyBranch('  --..hi')).toBe('hi');
  });

  it('collapses forbidden git sequences', () => {
    expect(slugifyBranch('a..b//c')).toBe('a.b/c');
  });

  it('keeps slashes for namespaced branches', () => {
    expect(slugifyBranch('feature/login form')).toBe('feature/login-form');
  });

  it('caps long input', () => {
    const out = slugifyBranch('a'.repeat(200));
    expect(out.length).toBeLessThanOrEqual(60);
  });
});

describe('finalizeBranchSlug', () => {
  it('strips trailing dash / dot / slash', () => {
    expect(finalizeBranchSlug('fix-login-')).toBe('fix-login');
    expect(finalizeBranchSlug('foo/')).toBe('foo');
    expect(finalizeBranchSlug('foo.')).toBe('foo');
  });

  it('is idempotent', () => {
    const a = finalizeBranchSlug('Fix the login bug');
    expect(finalizeBranchSlug(a)).toBe(a);
  });
});

describe('validateBranchName', () => {
  it('accepts ordinary names', () => {
    expect(validateBranchName('feat/login-fix')).toBeNull();
    expect(validateBranchName('fix-123')).toBeNull();
    expect(validateBranchName('release/v1.2.3')).toBeNull();
  });

  it('rejects whitespace and forbidden ref characters', () => {
    expect(validateBranchName('has space')).toMatch(/whitespace/);
    expect(validateBranchName('a~b')).toMatch(/cannot contain/);
    expect(validateBranchName('a:b')).toMatch(/cannot contain/);
    expect(validateBranchName('a*b')).toMatch(/cannot contain/);
    expect(validateBranchName('a[b')).toMatch(/cannot contain/);
    expect(validateBranchName('a\\b')).toMatch(/cannot contain/);
  });

  it('rejects bad leading / trailing / doubled sequences', () => {
    expect(validateBranchName('-x')).toMatch(/start with -/);
    expect(validateBranchName('/x')).toMatch(/start or end with/);
    expect(validateBranchName('x/')).toMatch(/start or end with/);
    expect(validateBranchName('x.')).toMatch(/end with/);
    expect(validateBranchName('x.lock')).toMatch(/end with/);
    expect(validateBranchName('a..b')).toMatch(/cannot contain \.\./);
    expect(validateBranchName('a//b')).toMatch(/cannot contain/);
    expect(validateBranchName('a@{b')).toMatch(/cannot contain/);
    expect(validateBranchName('a/.hidden')).toMatch(/start with a dot/);
    expect(validateBranchName('@')).toMatch(/not a valid/);
  });

  it('rejects an empty name', () => {
    expect(validateBranchName('')).toMatch(/required/);
    expect(validateBranchName('   ')).toMatch(/required/);
  });
});
