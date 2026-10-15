import { useState } from "preact/hooks";
import { MAX_DIFF_LINES } from "./config";
import { isDemoted } from "./derive";
import { Threads } from "./threads";
import {
  readClaim,
  setCursor,
  setFocus,
  threadsFor,
  toggleClaim,
  toggleMapping,
  toggleReviewed,
  mapKey,
  type Session,
} from "./session";
import type { Analysis, Hunk, Line, Peer } from "./types";

type Props = {
  hunk: Hunk;
  analysis: Analysis;
  session: Session;
  peers: Peer[];
  crits: string[];
  selected: string | null;
  defaultOpen: boolean;
};

function kindLabel(h: Hunk): string {
  switch (h.kind.type) {
    case "Moved":
      return "Moved";
    case "FormatOnly":
      return "Format only";
    case "New":
      return "New";
    case "Deleted":
      return "Deleted";
    default:
      return "Changed";
  }
}

/** New-side line number of each diff line (old-side for removed lines). */
function numbered(h: Hunk, lines: Line[]): number[] {
  let [o, n] = [h.old_start, h.new_start];
  return lines.map((l) => {
    if (l.kind === "-") return o++;
    o++;
    return n++;
  });
}

export function HunkCard({ hunk: h, analysis, session: s, peers, crits, selected, defaultOpen }: Props) {
  const [open, setOpen] = useState(defaultOpen);
  const [all, setAll] = useState(false);
  const [line, setLine] = useState<number | null>(null);
  const threads = threadsFor(s, h.id);
  const openThreads = threads.filter((t) => !t.resolvedBy).length;
  const me = s.provider.awareness.clientID;
  const cursors = peers.filter((p) => p.clientId !== me && p.cursor?.hunk === h.id);
  const demoted = isDemoted(h);
  const claim = readClaim(s.claims.get(h.id));
  const mine = claim?.user === s.me.name;
  const reviewer = s.reviewed.get(h.id);
  const viewers = peers.filter((p) => p.focus === h.id && p.clientId !== s.provider.awareness.clientID);
  const assumptions = analysis.assumptions.filter(
    (a) => a.path === h.path && (a.line == null || (a.line >= h.new_start && a.line < h.new_start + Math.max(h.new_len, 1))),
  );
  const syms = h.symbols.map((i) => analysis.symbols[i]).filter((x) => x !== undefined);
  const mappedHere = selected !== null && s.mappings.has(mapKey(selected, h.id));
  const lines = all ? h.lines : h.lines.slice(0, MAX_DIFF_LINES);
  const nums = numbered(h, lines);
  const bodyId = `body-${h.id}`;

  return (
    <article
      class={`hunk${demoted ? " demoted" : ""}${reviewer ? " reviewed" : ""}`}
      aria-label={`${h.path} line ${h.new_start}`}
      onMouseEnter={() => setFocus(s, h.id)}
      onFocusCapture={() => setFocus(s, h.id)}
      onMouseLeave={() => {
        setFocus(s, null);
        setCursor(s, null);
      }}
    >
      <header>
        <span class={`score ${h.score >= 40 ? "hi" : h.score >= 15 ? "mid" : "lo"}`} title="Risk score">{h.score}</span>
        <span class={`kind k-${h.kind.type}`}>{kindLabel(h)}</span>
        <button type="button" class="path mono" aria-expanded={open} aria-controls={bodyId} onClick={() => setOpen(!open)}>
          {h.path}:{h.new_start}
        </button>
        {viewers.map((p) => (
          <span key={p.clientId} class="viewing" style={{ background: p.color }} title={`${p.name} is viewing`} role="img" aria-label={`${p.name} is viewing`} />
        ))}
        <span class="spacer" />
        {openThreads > 0 && (
          <span class="chip threads-chip" title="Unresolved comment threads">
            {openThreads} {openThreads === 1 ? "thread" : "threads"}
          </span>
        )}
        {crits.map((c) => (
          <span key={c} class="chip crit-chip mono">{c}</span>
        ))}
      </header>
      {h.kind.type === "Moved" && (
        <p class="muted">
          Moved {h.lines.find((l) => l.kind !== " ")?.kind === "-" ? "to" : "from"}{" "}
          <span class="mono">{h.kind.from.path}:{h.kind.from.line}</span>
        </p>
      )}
      {h.signals.length > 0 && (
        <ul class="chips" aria-label="Risk reasons">
          {h.signals.map((sig, i) => (
            <li key={i} class="chip risk" title={`${sig.rule.type} (+${sig.points})`}>{sig.reason}</li>
          ))}
        </ul>
      )}
      {syms.length > 0 && (
        <p class="syms muted">
          Symbols:{" "}
          {syms.map((x, i) => (
            <span key={i} class="mono">{i > 0 ? ", " : ""}{x.name} ({x.change.type})</span>
          ))}
        </p>
      )}
      {assumptions.map((a, i) => (
        <p key={i} class="assumption">
          <b>Assumption{a.agent ? ` by ${a.agent}` : ""}:</b> {a.text}
          {a.line != null && <span class="muted"> (line {a.line})</span>}
        </p>
      ))}
      <div class="actions">
        {selected && (
          <button type="button" class={mappedHere ? "on" : ""} aria-pressed={mappedHere} onClick={() => toggleMapping(s, selected, h.id)}>
            {mappedHere ? `Mapped to ${selected}` : `Map to ${selected}`}
          </button>
        )}
        <button type="button" aria-pressed={mine} disabled={!!claim && !mine} onClick={() => toggleClaim(s, h.id)}>
          {mine ? "Release" : "Claim"}
        </button>
        <button type="button" class={reviewer ? "on" : ""} aria-pressed={!!reviewer} onClick={() => toggleReviewed(s, h.id)}>
          {reviewer ? "Reviewed" : "Mark reviewed"}
        </button>
        {claim && <span class="label">Claimed by {claim.user}</span>}
        {reviewer && <span class="label">Reviewed by {reviewer}</span>}
      </div>
      {open && (
        <div id={bodyId}>
          <pre class="diff" tabIndex={0} aria-label="Diff. Click a line to comment on it.">
            {lines.map((l, i) => {
              const no = nums[i] ?? 0;
              const here = cursors.filter((p) => l.kind !== "-" && p.cursor?.line === no);
              const cls = l.kind === "+" ? "add" : l.kind === "-" ? "del" : "ctx";
              return (
                <span
                  key={i}
                  class={`${cls}${line === no && l.kind !== "-" ? " picked" : ""}`}
                  style={here[0] ? { boxShadow: `inset 3px 0 0 ${here[0].color}` } : undefined}
                  title={here.length ? `${here.map((p) => p.name).join(", ")} here` : undefined}
                  onMouseEnter={() => l.kind !== "-" && setCursor(s, { hunk: h.id, line: no })}
                  onClick={() => l.kind !== "-" && setLine(no)}
                >
                  <span class="ln" aria-hidden="true">{l.kind === "-" ? "" : no}</span>
                  {l.kind}{l.text}{"\n"}
                </span>
              );
            })}
          </pre>
          {h.lines.length > lines.length && (
            <button type="button" onClick={() => setAll(true)}>Show all {h.lines.length} lines</button>
          )}
        </div>
      )}
      {(open || threads.length > 0) && (
        <Threads session={s} hunk={h.id} threads={threads} line={line} onClearLine={() => setLine(null)} />
      )}
    </article>
  );
}
