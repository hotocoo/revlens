import { WebsocketProvider } from "y-websocket";
import * as Y from "yjs";
import { NAME_STORAGE_KEY, PRESENCE_COLORS } from "./config";
import type { Claim, Peer, Status } from "./types";

/** Doc layout is defined in src/live.rs; keep these in sync. */
export type Session = {
  doc: Y.Doc;
  provider: WebsocketProvider;
  status: Y.Map<string>;
  mappings: Y.Map<string>;
  reviewed: Y.Map<string>;
  claims: Y.Map<string>;
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
  return {
    doc,
    provider,
    status: doc.getMap("status"),
    mappings: doc.getMap("mappings"),
    reviewed: doc.getMap("reviewed"),
    claims: doc.getMap("claims"),
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
    out.push({ clientId, name: u.name, color: u.color, focus: (st.focus as string | null) ?? null });
  }
  return out;
}
