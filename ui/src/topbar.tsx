import { IconDownload, IconKeyboard, IconLayoutSidebarLeftExpand } from "@tabler/icons-preact";
import { shortSha, type Derived } from "./derive";
import type { Analysis, Peer } from "./types";

type Props = {
  analysis: Analysis;
  derived: Derived;
  peers: Peer[];
  connected: boolean;
  token: string;
  /** Present when criteria live in a drawer (narrow screens). */
  onOpenCriteria?: () => void;
  onShortcuts: () => void;
};

function Meter({ label, done, total }: { label: string; done: number; total: number }) {
  const p = total ? done / total : 0;
  return (
    <div class="meter" role="group" aria-label={`${label}: ${done} of ${total}`}>
      <div class="meter-head">
        <span class="meter-label">{label}</span>
        <span class="meter-num num">
          <b>{done}</b>/{total}
        </span>
      </div>
      <div class="meter-track" aria-hidden="true">
        <span class={`meter-fill${p === 1 ? " full" : ""}`} style={{ transform: `scaleX(${p})` }} />
      </div>
    </div>
  );
}

export function TopBar({ analysis, derived: d, peers, connected, token, onOpenCriteria, onShortcuts }: Props) {
  const t = encodeURIComponent(token);
  const unique = [...new Map(peers.map((p) => [p.name, p])).values()];
  const r = analysis.range;
  return (
    <header class="bar">
      <div class="bar-id">
        {onOpenCriteria && (
          <button type="button" class="btn ghost icon" onClick={onOpenCriteria} aria-label="Show criteria">
            <IconLayoutSidebarLeftExpand size={18} stroke={1.75} />
          </button>
        )}
        <span class="brand" aria-label="Lens">
          <span class="brand-mark" aria-hidden="true" />
          Lens
        </span>
        <span class="range mono" title={`${r.base}..${r.head} (merge base ${r.merge_base})`}>
          {shortSha(r.merge_base)}
          <span class="range-sep">..</span>
          {shortSha(r.head)}
        </span>
      </div>

      <div class="bar-progress">
        <Meter label="Criteria met" done={d.metCount} total={analysis.criteria.length} />
        <Meter label="Reviewed" done={d.reviewedCount} total={analysis.hunks.length} />
        <ul class="flags" aria-label="Open issues">
          <li class={d.scopeCreep ? "flag warn" : "flag"} title="Hunks mapped to no criterion">
            <b class="num">{d.scopeCreep}</b> scope creep
          </li>
          <li class={d.missingWork.length ? "flag warn" : "flag"} title="Criteria with no hunks">
            <b class="num">{d.missingWork.length}</b> missing
          </li>
        </ul>
      </div>

      <div class="bar-end">
        <div class="presence">
          <span class={connected ? "live on" : "live"} role="status">
            <span class="live-dot" aria-hidden="true" />
            {connected ? "Live" : "Offline"}
          </span>
          <ul class="avatars" aria-label={`${unique.length} reviewers online`}>
            {unique.map((p) => (
              <li key={p.name} class="avatar" style={{ "--c": p.color }} title={p.name} aria-label={p.name}>
                {p.name.slice(0, 1).toUpperCase()}
              </li>
            ))}
          </ul>
        </div>
        <button type="button" class="btn ghost icon" onClick={onShortcuts} aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)">
          <IconKeyboard size={18} stroke={1.75} />
        </button>
        <details class="popover">
          <summary class="btn">
            <IconDownload size={16} stroke={1.75} />
            Export
          </summary>
          <div class="popover-panel" role="menu">
            <a role="menuitem" href={`/api/export.md?t=${t}`}>
              Markdown<span class="muted">conformance record</span>
            </a>
            <a role="menuitem" href={`/api/export.json?t=${t}`}>
              JSON<span class="muted">schema 1</span>
            </a>
          </div>
        </details>
      </div>
    </header>
  );
}
