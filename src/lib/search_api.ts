// Search everything (search phase 3): the hub's full-text index over tasks
// and tickets, sessions, conversations' first prompts, pull requests, the
// work journal and — when the hub indexes transcripts — what was said in
// each conversation. Every hit is fenced for this device's person on the
// hub (`service::search`).
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export const SEARCH_KINDS = ['item', 'session', 'conversation', 'pr', 'journal', 'transcript'] as const;
export type SearchKind = (typeof SEARCH_KINDS)[number];

export interface SearchHit {
  kind: SearchKind | string;
  /** The source row's id (a transcript chunk: `<claude id>:<offset>`). */
  ref: string;
  title: string;
  /** `[start, end)` in UTF-16 units: what JavaScript strings index by. */
  title_marks?: [number, number][];
  snippet: string;
  snippet_marks?: [number, number][];
  at: number;
  session_id?: number | null;
  session_name?: string | null;
  host_alias?: string | null;
  claude_session_id?: string | null;
  /** An item hit: `item:<id>` and its key. */
  task_id?: string | null;
  key?: string | null;
}

export interface SearchPage {
  hits: SearchHit[];
  /** The hub copies conversation text into the index (an owner's setting,
   *  off by default); without it a conversation is found by its first
   *  prompt and its journal. */
  transcripts_indexed: boolean;
}

export interface SearchQuery {
  query: string;
  kinds?: SearchKind[];
  limit?: number;
}

export function searchEverything(q: SearchQuery): Promise<Result<SearchPage>> {
  return invokeCmd<SearchPage>('search', { args: q });
}

/** A request for a session's Conversation tab to open Find on a query
 *  (a ⌘K hit in what was said): the panel showing that session takes it. */
export const conversationFindRequest = writable<{ sessionId: number; query: string; seq: number } | null>(null);
let findSeq = 0;
export function requestConversationFind(sessionId: number, query: string): void {
  conversationFindRequest.set({ sessionId, query, seq: ++findSeq });
}
