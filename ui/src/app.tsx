import { useEffect, useMemo, useState } from "preact/hooks";
import { Criteria } from "./criteria";
import { derive, isDemoted, shortSha } from "./derive";
import { HunkCard } from "./hunk-card";
import { peers, type Session } from "./session";
import type { Analysis } from "./types";
import { OPEN_BY_DEFAULT } from "./config";

/** Re-render whenever shared state or presence changes. */
function useRevision(s: Session): number {
  const [rev, setRev] = useState(0);
  useEffect(() => {
    const bump = () => setRev((r) => r + 1);
    const maps = [s.status, s.mappings, s.reviewed, s.claims];
    maps.forEach((m) => m.observe(bump));
    s.provider.awareness.on("change", bump);
    s.provider.on("status", bump);
    return () => {
      maps.forEach((m) => m.unobserve(bump));
      s.provider.awareness.off("change", bump);
      s.provider.off("status", bump);
    };
  }, [s]);
  return rev;
}

export function App({ analysis, session, token }: { analysis: Analysis; session: Session; token: string }) {
  useRevision(session);
  const [selected, setSelected] = useState<string | null>(null);
  const [showDemoted, setShowDemoted] = useState(true);
  const [filter, setFilter] = useState("");
  const d = derive(analysis, session);
  const ps = peers(session);
  const unique = [...new Map(ps.map((p) => [p.name, p])).values()];
  const hunks = analysis.hunks;
  const initialOpen = useMemo(
    () => new Set(hunks.filter((h) => !isDemoted(h)).slice(0, OPEN_BY_DEFAULT).map((h) => h.id)),
    [hunks],
  );
  const visible = hunks.filter(
    (h) => (showDemoted || !isDemoted(h)) && h.path.toLowerCase().includes(filter.toLowerCase()),
  );
  const generalAssumptions = analysis.assumptions.filter((a) => !a.path);
  const t = encodeURIComponent(token);
  const connected = session.provider.wsconnected;

  return (
    <div class="app">
      <header class="top">
        <div class="range">
          <h1>Lens</h1>
          <span class="mono" title={`${analysis.range.base}..${analysis.range.head}`}>
            {shortSha(analysis.range.base)}..{shortSha(analysis.range.head)}
          </span>
          <span class="muted">merge-base <span class="mono">{shortSha(analysis.range.merge_base)}</span></span>
        </div>
        <ul class="counts" aria-label="Review progress">
          <li>Criteria met <b>{d.metCount}/{analysis.criteria.length}</b></li>
          <li>Hunks reviewed <b>{d.reviewedCount}/{hunks.length}</b></li>
          <li class={d.scopeCreep ? "warn" : ""}>Scope creep <b>{d.scopeCreep}</b></li>
          <li class={d.missingWork.length ? "warn" : ""}>Missing work <b>{d.missingWork.length}</b></li>
        </ul>
        <div class="presence">
          <span class={connected ? "conn on" : "conn"} role="status">{connected ? "Live" : "Offline"}</span>
          <ul class="avatars" aria-label="Reviewers online">
            {unique.map((p) => (
              <li key={p.name} class="avatar" style={{ background: p.color }} title={p.name} aria-label={p.name}>
                {p.name.slice(0, 1).toUpperCase()}
              </li>
            ))}
          </ul>
        </div>
        <details class="export">
          <summary>Export</summary>
          <div class="menu">
            <a href={`/api/export.md?t=${t}`}>Markdown</a>
            <a href={`/api/export.json?t=${t}`}>JSON</a>
          </div>
        </details>
      </header>
      <main class="cols">
        <Criteria
          analysis={analysis}
          session={session}
          derived={d}
          selected={selected}
          onSelect={setSelected}
          general={generalAssumptions}
        />
        <section class="hunks" aria-label="Hunks ranked by risk">
          <div class="filters">
            <label for="filter">Filter by path</label>
            <input id="filter" type="search" value={filter} onInput={(e) => setFilter(e.currentTarget.value)} />
            <label class="check">
              <input type="checkbox" checked={showDemoted} onChange={(e) => setShowDemoted(e.currentTarget.checked)} />
              Show moved and format-only hunks
            </label>
            <span class="muted">{visible.length} of {hunks.length}</span>
          </div>
          {selected && (
            <p class="mapmode" role="status">
              Map mode: choose hunks for <b>{selected}</b>.{" "}
              <button type="button" onClick={() => setSelected(null)}>Done</button>
            </p>
          )}
          {visible.map((h) => (
            <HunkCard
              key={h.id}
              hunk={h}
              analysis={analysis}
              session={session}
              peers={ps}
              crits={d.critsByHunk.get(h.id) ?? []}
              selected={selected}
              defaultOpen={initialOpen.has(h.id)}
            />
          ))}
          {visible.length === 0 && <p class="muted">No hunks match.</p>}
        </section>
      </main>
    </div>
  );
}
