import {
  IconArrowsShuffle,
  IconCheck,
  IconChevronRight,
  IconHandGrab,
  IconLink,
  IconMessageCircle,
  IconRobot,
} from "@tabler/icons-preact";
import { useState } from "preact/hooks";
import { MAX_DIFF_LINES } from "./config";
import { isDemoted } from "./derive";
import { Threads } from "./threads";
import {
  mapKey,
  readClaim,
  setCursor,
  setFocus,
  threadsFor,
  toggleClaim,
  toggleMapping,
  toggleReviewed,
  type Session,
} from "./session";
import type { Analysis, Hunk, Line, Peer } from "./types";

type Props = {
  index: number;
  hunk: Hunk;
  analysis: Analysis;
  session: Session;
  peers: Peer[];
  crits: string[];
  selected: string | null;
  defaultOpen: boolean;
};

const KIND: Record<Hunk["kind"]["type"], string> = {
  Changed: "Changed",
  New: "New",
  Deleted: "Deleted",
  Moved: "Moved",
  FormatOnly: "Format only",
};

function band(score: number): "hi" | "mid" | "lo" {
  return score >= 40 ? "hi" : score >= 15 ? "mid" : "lo";
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

function splitPath(path: string): [string, string] {
  const i = path.lastIndexOf("/");
  return i < 0 ? ["", path] : [path.slice(0, i + 1), path.slice(i + 1)];
}

export function HunkCard({ index, hunk: h, analysis, session: s, peers, crits, selected, defaultOpen }: Props) {
  const [open, setOpen] = useState(defaultOpen);
  const [all, setAll] = useState(false);
  const [line, setLine] = useState<number | null>(null);
  const demoted = isDemoted(h);
  const claim = readClaim(s.claims.get(h.id));
  const mine = claim?.user === s.me.name;
  const reviewer = s.reviewed.get(h.id);
  const me = s.provider.awareness.clientID;
  const viewers = peers.filter((p) => p.focus === h.id && p.clientId !== me);
  const cursors = peers.filter((p) => p.clientId !== me && p.cursor?.hunk === h.id);
  const threads = threadsFor(s, h.id);
  const openThreads = threads.filter((t) => !t.resolvedBy).length;
  const assumptions = analysis.assumptions.filter(
    (a) => a.path === h.path && (a.line == null || (a.line >= h.new_start && a.line < h.new_start + Math.max(h.new_len, 1))),
  );
  const syms = h.symbols.map((i) => analysis.symbols[i]).filter((x) => x !== undefined);
  const mappedHere = selected !== null && s.mappings.has(mapKey(selected, h.id));
  const lines = all ? h.lines : h.lines.slice(0, MAX_DIFF_LINES);
  const nums = numbered(h, lines);
  const [dir, file] = splitPath(h.path);
  const bodyId = `body-${h.id}`;
  const b = band(h.score);

  return (
    <li
      class={`card reveal band-${b}${demoted ? " demoted" : ""}${reviewer ? " reviewed" : ""}${mappedHere ? " mapped" : ""}`}
      data-hunk={h.id}
      style={{ "--d": Math.min(index, 10) }}
      onMouseEnter={() => setFocus(s, h.id)}
      onFocusCapture={() => setFocus(s, h.id)}
      onMouseLeave={() => {
        setFocus(s, null);
        setCursor(s, null);
      }}
    >
      <header class="card-head">
        <span class="score num" title={`Risk score ${h.score}`} aria-label={`Risk ${h.score}`}>
          {h.score}
        </span>
        <div class="card-title">
          <button
            type="button"
            class="path"
            data-act="toggle"
            aria-expanded={open}
            aria-controls={bodyId}
            onClick={() => setOpen(!open)}
          >
            <IconChevronRight class="chev" size={16} stroke={2} aria-hidden="true" />
            <span class="mono">
              <span class="dir">{dir}</span>
              <span class="file">{file}</span>
              <span class="at">:{h.new_start}</span>
            </span>
          </button>
          <p class="card-meta">
            <span class={`kind k-${h.kind.type}`}>{KIND[h.kind.type]}</span>
            {h.kind.type === "Moved" && (
              <span class="muted">
                <IconArrowsShuffle size={13} stroke={1.75} aria-hidden="true" />{" "}
                {h.lines.find((l) => l.kind !== " ")?.kind === "-" ? "to" : "from"}{" "}
                <span class="mono">
                  {h.kind.from.path}:{h.kind.from.line}
                </span>
              </span>
            )}
            {syms.map((x, i) => (
              <span key={i} class="sym mono" title={x.change.type}>
                {x.name}
              </span>
            ))}
          </p>
        </div>
        <div class="card-side">
          {viewers.length > 0 && (
            <span class="viewers" role="img" aria-label={`Viewing: ${viewers.map((p) => p.name).join(", ")}`}>
              {viewers.map((p) => (
                <span key={p.clientId} class="viewer" style={{ "--c": p.color }} title={`${p.name} is here`} />
              ))}
            </span>
          )}
          {openThreads > 0 && (
            <span class="pill warn" title="Unresolved comment threads">
              <IconMessageCircle size={13} stroke={2} aria-hidden="true" />
              {openThreads} {openThreads === 1 ? "thread" : "threads"}
            </span>
          )}
          {crits.map((c) => (
            <span key={c} class="pill accent mono">
              {c}
            </span>
          ))}
        </div>
      </header>

      {h.signals.length > 0 && (
        <ul class="reasons" aria-label="Why this ranks here">
          {h.signals.map((sig, i) => (
            <li key={i} title={sig.rule.type}>
              <span class="pts num">+{sig.points}</span>
              {sig.reason}
            </li>
          ))}
        </ul>
      )}

      {assumptions.map((a, i) => (
        <p key={i} class="assumption">
          <IconRobot size={15} stroke={1.75} aria-hidden="true" />
          <span>
            <b>{a.agent ? `${a.agent} assumed` : "Assumed"}</b> {a.text}
            {a.line != null && <span class="muted"> line {a.line}</span>}
          </span>
        </p>
      ))}

      <div class="actions">
        {selected && (
          <button type="button" class="btn sm toggle" data-act="map" aria-pressed={mappedHere} onClick={() => toggleMapping(s, selected, h.id)}>
            <IconLink size={14} stroke={1.75} aria-hidden="true" />
            {mappedHere ? `Mapped to ${selected}` : `Map to ${selected}`}
          </button>
        )}
        <button
          type="button"
          class="btn sm"
          data-act="claim"
          aria-pressed={mine}
          disabled={!!claim && !mine}
          onClick={() => toggleClaim(s, h.id)}
        >
          <IconHandGrab size={14} stroke={1.75} aria-hidden="true" />
          {mine ? "Release" : "Claim"}
        </button>
        <button type="button" class="btn sm ok" data-act="review" aria-pressed={!!reviewer} onClick={() => toggleReviewed(s, h.id)}>
          <IconCheck size={14} stroke={2.25} aria-hidden="true" key={reviewer ? "on" : "off"} />
          {reviewer ? "Reviewed" : "Mark reviewed"}
        </button>
        <span class="who">
          {claim && <span>Claimed by {claim.user}</span>}
          {reviewer && <span>Reviewed by {reviewer}</span>}
        </span>
      </div>

      {open && (
        <div id={bodyId} class="card-body">
          <pre class="diff" tabIndex={0} aria-label="Diff. Click a line to comment on it.">
            {lines.map((l, i) => {
              const no = nums[i] ?? 0;
              const peer = l.kind === "-" ? undefined : cursors.find((p) => p.cursor?.line === no);
              const cls = l.kind === "+" ? "add" : l.kind === "-" ? "del" : "ctx";
              return (
                <span
                  key={i}
                  class={`dl ${cls}${line === no && l.kind !== "-" ? " picked" : ""}${peer ? " peer" : ""}`}
                  style={peer ? { "--c": peer.color } : undefined}
                  data-peer={peer?.name}
                  onMouseEnter={() => l.kind !== "-" && setCursor(s, { hunk: h.id, line: no })}
                  onClick={() => l.kind !== "-" && setLine(no)}
                >
                  <span class="ln" aria-hidden="true">
                    {no}
                  </span>
                  <span class="mk" aria-hidden="true">
                    {l.kind}
                  </span>
                  {l.text}
                  {"\n"}
                </span>
              );
            })}
          </pre>
          {h.lines.length > lines.length && (
            <button type="button" class="btn sm" onClick={() => setAll(true)}>
              Show all {h.lines.length} lines
            </button>
          )}
        </div>
      )}
      {(open || threads.length > 0) && (
        <Threads
          session={s}
          hunk={h.id}
          threads={threads}
          line={line}
          lines={[h.new_start, h.new_start + Math.max(h.new_len, 1) - 1]}
          onLine={setLine}
        />
      )}
    </li>
  );
}
