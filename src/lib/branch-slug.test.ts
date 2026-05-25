import { describe, it, expect } from 'vitest';
import { slugifyBranch, finalizeBranchSlug } from './branch-slug';

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
