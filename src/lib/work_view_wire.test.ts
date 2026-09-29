// The Work view against what the hub REALLY serialises (work graph M14):
// `__fixtures__/work_view_wire/*.json` are dumped by
// `WORK_VIEW_FIXTURE_DIR=<dir> cargo test -p fleet-core --lib dump_wire_samples`.
// The hand-written fixtures prove the screens; these prove the types and the
// helpers read the real shapes. Regenerate them when the hub's shapes change.
import { describe, expect, it } from 'vitest';
import tree from './__fixtures__/work_view_wire/tree.json';
import treeSections from './__fixtures__/work_view_wire/tree_sections.json';
import task from './__fixtures__/work_view_wire/task.json';
import sessionTasks from './__fixtures__/work_view_wire/session_tasks.json';
import review from './__fixtures__/work_view_wire/review.json';
import orgImpact from './__fixtures__/work_view_wire/org_impact.json';
import {
  buildSections,
  distributeTasks,
  occurrenceKind,
  sectionKey,
  type OrgImpact,
  type ReviewPage,
  type SessionTasks,
  type TaskDetail,
  type WorkTreePage,
  type WorkTreeSection,
} from './work_view';

describe('work view: the hub’s real wire shapes', () => {
  const page = tree as unknown as WorkTreePage;

  it('a tree page builds its sections with every header and task', () => {
    const sections = buildSections(page.groups, page.orgs, distributeTasks(page));
    const acme = sections.find((s) => s.name === 'Acme');
    expect(acme).toBeDefined();
    const labels = acme!.groups.map((g) => g.group.label);
    expect(labels).toEqual(expect.arrayContaining(['Security', 'TP']));
    const total = acme!.groups.reduce((n, g) => n + g.tasks.length, 0);
    expect(total).toBe(page.tasks.filter((t) => t.org_id === acme!.orgId).length);
  });

  it('one session under two tasks is one identity; exactly one is primary', () => {
    const tk1 = page.tasks.find((t) => t.key === 'TK-1')!;
    const tk2 = page.tasks.find((t) => t.key === 'TK-2')!;
    expect(tk1.sessions![0].session_id).toBe(tk2.sessions![0].session_id);
    expect(occurrenceKind(tk1.sessions![0])).toBe('primary');
    expect(occurrenceKind(tk2.sessions![0])).toBe('secondary');
    expect(tk2.group.source).toBe('manual');
    expect(tk2.group.tracker_value).toBe('TP');
    const tk3 = page.tasks.find((t) => t.key === 'TK-3')!;
    expect(occurrenceKind(tk3.sessions![0])).toBe('suggested');
  });

  it('a tree read pages its open sections and the review total as WorkTree reads them', () => {
    const p = treeSections as unknown as WorkTreePage;
    // WorkTree keys the paged sections by org and group, then matches them
    // to the page's group headers.
    const paged = new Map<string, WorkTreeSection>();
    expect(Array.isArray(p.sections)).toBe(true);
    for (const sec of p.sections!) paged.set(sectionKey(sec.org_id, sec.group_id), sec);
    const tp = p.groups.find((g) => g.group.label === 'TP')!;
    const sec = paged.get(sectionKey(tp.org_id, tp.group.id))!;
    expect(sec).toBeDefined();
    expect(Array.isArray(sec.tasks)).toBe(true);
    expect(sec.tasks.map((t) => t.key)).toEqual(['TK-1']);
    expect(sec.tasks[0].group.id).toBe(tp.group.id);
    // Asked for one of the section's two tasks: a cursor to page on.
    expect(tp.count).toBe(2);
    expect(typeof (sec.next_cursor ?? null)).toBe('string');
    expect(typeof p.review_total).toBe('number');
    expect(p.review_total).toBe((review as unknown as ReviewPage).total);
    // Asked for neither, the plain read has neither.
    expect(page.sections).toBeUndefined();
    expect(page.review_total).toBeUndefined();
  });

  it('a task detail carries its placement', () => {
    const d = task as unknown as TaskDetail;
    expect(d.task.task_id).toBe('item:2');
    expect(d.placement?.group).toBe('Security');
    expect(d.placement?.version).toBe(1);
  });

  it("session tasks carry each link's task", () => {
    const st = sessionTasks as unknown as SessionTasks;
    expect(st.links.map((l) => l.task.key).sort()).toEqual(['TK-1', 'TK-2']);
    expect(st.links.filter((l) => l.primary)).toHaveLength(1);
  });

  it('a review item says why, and an org impact what changes', () => {
    const r = review as unknown as ReviewPage;
    const s = r.items.find((i) => i.kind === 'suggestion')!;
    expect(s.why!.length).toBeGreaterThan(0);
    const i = orgImpact as unknown as OrgImpact;
    expect(i.allowed).toBe(true);
    expect(i.links[0].becomes_cross_org).toBe(true);
    expect(i.impact_token).toBeTruthy();
  });
});
