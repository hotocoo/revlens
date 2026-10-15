export type Loc = { path: string; line: number };
export type HunkKind =
  | { type: "Changed" | "New" | "Deleted" | "FormatOnly" }
  | { type: "Moved"; from: Loc };
export type Line = { kind: " " | "+" | "-"; text: string };
export type Signal = { rule: { type: string }; points: number; reason: string };
export type Hunk = {
  id: string;
  path: string;
  old_start: number;
  old_len: number;
  new_start: number;
  new_len: number;
  lines: Line[];
  kind: HunkKind;
  symbols: number[];
  signals: Signal[];
  score: number;
};
export type SymbolChange = { name: string; kind: string; path: string; change: { type: string } };
export type Criterion = { id: string; text: string; source_line: number };
export type Assumption = {
  text: string;
  agent?: string | null;
  path?: string | null;
  line?: number | null;
};
export type Analysis = {
  /** Websocket room, added by the server. */
  room: string;
  range: { base: string; head: string; merge_base: string };
  hunks: Hunk[];
  symbols: SymbolChange[];
  criteria: Criterion[];
  assumptions: Assumption[];
};
export type Status = "Open" | "Met" | "NotMet" | "NotApplicable";
export type Claim = { user: string; at_ms: number };
export type Cursor = { hunk: string; line: number };
export type Peer = { clientId: number; name: string; color: string; focus: string | null; cursor: Cursor | null };
/** Mirrors `Comment` in src/model.rs. */
export type Comment = {
  hunk: string;
  line: number | null;
  parent: string | null;
  user: string;
  text: string;
  at_ms: number;
};
export type Thread = { id: string; root: Comment; replies: [string, Comment][]; resolvedBy: string | null };
