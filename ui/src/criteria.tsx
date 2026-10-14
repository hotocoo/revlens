import { setStatus, type Session } from "./session";
import type { Derived } from "./derive";
import type { Analysis, Assumption, Status } from "./types";

const STATUSES: [Status, string][] = [
  ["Open", "Open"],
  ["Met", "Met"],
  ["NotMet", "Not met"],
  ["NotApplicable", "N/A"],
];

type Props = {
  analysis: Analysis;
  session: Session;
  derived: Derived;
  selected: string | null;
  onSelect: (id: string | null) => void;
  general: Assumption[];
};

export function Criteria({ analysis, session, derived, selected, onSelect, general }: Props) {
  return (
    <aside class="criteria" aria-label="Acceptance criteria">
      <h2>Criteria</h2>
      {analysis.criteria.length === 0 && (
        <p class="muted">No criteria. Pass --intent FILE or --pr N to review against intent.</p>
      )}
      <ol>
        {analysis.criteria.map((c) => {
          const status = (session.status.get(c.id) as Status | undefined) ?? "Open";
          const mapped = derived.hunksByCrit.get(c.id)?.size ?? 0;
          const missing = derived.missingWork.includes(c.id);
          const on = selected === c.id;
          return (
            <li key={c.id} class={on ? "crit selected" : "crit"}>
              <button
                type="button"
                class="crit-text"
                aria-pressed={on}
                onClick={() => onSelect(on ? null : c.id)}
                title="Select to map hunks to this criterion"
              >
                <span class="mono id">{c.id}</span> {c.text}
              </button>
              <div class="crit-meta">
                <span class="muted">{mapped} hunk{mapped === 1 ? "" : "s"}</span>
                {missing && <span class="badge warn">Missing work</span>}
              </div>
              <div class="seg" role="group" aria-label={`Status of ${c.id}`}>
                {STATUSES.map(([v, label]) => (
                  <button
                    key={v}
                    type="button"
                    class={status === v ? `on s-${v}` : ""}
                    aria-pressed={status === v}
                    onClick={() => setStatus(session, c.id, v)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </li>
          );
        })}
      </ol>
      {general.length > 0 && (
        <>
          <h2>Agent assumptions</h2>
          <ul class="assumptions">
            {general.map((a, i) => (
              <li key={i}>
                {a.text}
                {a.agent && <span class="muted"> ({a.agent})</span>}
              </li>
            ))}
          </ul>
        </>
      )}
    </aside>
  );
}
