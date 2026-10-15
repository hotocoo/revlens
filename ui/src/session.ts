import { WebsocketProvider } from "y-websocket";
import * as Y from "yjs";
import { NAME_STORAGE_KEY, PRESENCE_COLORS } from "./config";
import type { Claim, Comment, Cursor, Peer, Status, Thread } from "./types";

/** Doc layout is defined in src/live.rs; keep these in sync. */
export type Session = {
  doc: Y.Doc;
  provider: WebsocketProvider;
  status: Y.Map<string>;
  mappings: Y.Map<string>;
  reviewed: Y.Map<string>;
  claims: Y.Map<string>;
  comments: Y.Map<string>;
  resolved: Y.Map<string>;
  me: { name: string; color: string };
};

export function colorFor(name: string): string {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return PRESENCE_COLORS[h % PRESENCE_COLORS.length] ?? "#0b7285";
}

export function storedName(): string {
  try {
    return localStorage.getItem(NAME_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

export function storeName(name: string) {
  try {
    localStorage.setItem(NAME_STORAGE_KEY, name);
  } catch {
    /* private mode: the name just is not remembered */
  }
}

export function wsUrl(origin: string): string {
  return origin.replace(/^http/, "ws") + "/ws";
}

export function connect(room: string, token: string, name: string): Session {
  const doc = new Y.Doc();
  const provider = new WebsocketProvider(wsUrl(location.origin), room, doc, { params: { t: token } });
  const me = { name, color: colorFor(name) };
  provider.awareness.setLocalStateField("user", me);
  provider.awareness.setLocalStateField("focus", null);
  provider.awareness.setLocalStateField("cursor", null);
  return {
    doc,
    provider,
    status: doc.getMap("status"),
    mappings: doc.getMap("mappings"),
    reviewed: doc.getMap("reviewed"),
    claims: doc.getMap("claims"),
    comments: doc.getMap("comments"),
    resolved: doc.getMap("resolved"),
    me,
  };
}

export const mapKey = (crit: string, hunk: string) => `${crit}|${hunk}`;

export function setStatus(s: Session, crit: string, v: Status) {
  s.status.set(crit, v);
}

export function toggleMapping(s: Session, crit: string, hunk: string) {
  const k = mapKey(crit, hunk);
  if (s.mappings.has(k)) s.mappings.delete(k);
  else s.mappings.set(k, s.me.name);
}

export function toggleReviewed(s: Session, hunk: string) {
  if (s.reviewed.has(hunk)) s.reviewed.delete(hunk);
  else s.reviewed.set(hunk, s.me.name);
}

export function toggleClaim(s: Session, hunk: string) {
  const mine = readClaim(s.claims.get(hunk))?.user === s.me.name;
  if (s.claims.has(hunk) && !mine) return;
  if (mine) s.claims.delete(hunk);
  else s.claims.set(hunk, JSON.stringify({ user: s.me.name, at_ms: Date.now() } satisfies Claim));
}

export function readClaim(raw: string | undefined): Claim | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as Claim;
  } catch {
    return null;
  }
}

export function setFocus(s: Session, hunk: string | null) {
  s.provider.awareness.setLocalStateField("focus", hunk);
}

export function peers(s: Session): Peer[] {
  const out: Peer[] = [];
  for (const [clientId, st] of s.provider.awareness.getStates()) {
    const u = st.user as { name: string; color: string } | undefined;
    if (!u) continue;
    out.push({
      clientId,
      name: u.name,
      color: u.color,
      focus: (st.focus as string | null) ?? null,
      cursor: (st.cursor as Cursor | null) ?? null,
    });
  }
  return out;
}

export function setCursor(s: Session, cursor: Cursor | null) {
  s.provider.awareness.setLocalStateField("cursor", cursor);
}

let seq = 0;
/** Unique without crypto.randomUUID, which plain-HTTP LAN pages (--share) do not have. */
function commentId(s: Session): string {
  seq += 1;
  return `${s.doc.clientID.toString(36)}-${Date.now().toString(36)}-${seq}`;
}

/** Start a thread (`parent` null) or reply to one. Empty text is ignored. */
export function addComment(s: Session, hunk: string, line: number | null, parent: string | null, text: string) {
  const t = text.trim();
  if (!t) return;
  const c: Comment = { hunk, line, parent, user: s.me.name, text: t, at_ms: Date.now() };
  s.comments.set(commentId(s), JSON.stringify(c));
}

export function toggleResolved(s: Session, root: string) {
  if (s.resolved.has(root)) s.resolved.delete(root);
  else s.resolved.set(root, s.me.name);
}

export function readComment(raw: string | undefined): Comment | null {
  if (!raw) return null;
  try {
    const c = JSON.parse(raw) as Comment;
    return typeof c.text === "string" && typeof c.user === "string" && typeof c.hunk === "string" ? c : null;
  } catch {
    return null;
  }
}

/** Threads on one hunk, oldest first, replies in time order. */
export function threadsFor(s: Session, hunk: string): Thread[] {
  const all: [string, Comment][] = [];
  for (const [id, raw] of s.comments.entries()) {
    const c = readComment(raw);
    if (c && c.hunk === hunk) all.push([id, c]);
  }
  all.sort((a, b) => a[1].at_ms - b[1].at_ms || a[0].localeCompare(b[0]));
  return all
    .filter(([, c]) => !c.parent)
    .map(([id, root]) => ({
      id,
      root,
      replies: all.filter(([, c]) => c.parent === id),
      resolvedBy: s.resolved.get(id) ?? null,
    }));
}
