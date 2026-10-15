import { IconLink, IconRobot } from "@tabler/icons-preact";
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
  const met = derived.metCount;
  return (
    <div class="criteria">
      <div class="section-head">
        <h2>Criteria</h2>
        <span class="muted num">
          {met} of {analysis.criteria.length} met
        </span>
      </div>
      {analysis.criteria.length === 0 && (
        <p class="hint">
          No criteria yet. Run with <code>--intent FILE</code> or <code>--pr N</code> to review against what the change
          was meant to do.
        </p>
      )}
      <ol class="crits">
        {analysis.criteria.map((c, i) => {
          const status = (session.status.get(c.id) as Status | undefined) ?? "Open";
          const idx = STATUSES.findIndex(([v]) => v === status);
          const mapped = derived.hunksByCrit.get(c.id)?.size ?? 0;
          const missing = derived.missingWork.includes(c.id);
          const on = selected === c.id;
          return (
            <li key={c.id} class={`crit enter${on ? " selected" : ""}`} data-status={status} style={{ "--d": i }}>
              <p class="crit-text">
                <span class="crit-id mono">{c.id}</span>
                {c.text}
              </p>
              <div class="crit-foot">
                <span class={`crit-count num${missing ? " warn" : ""}`}>
                  {missing ? "No hunks yet" : `${mapped} hunk${mapped === 1 ? "" : "s"}`}
                </span>
                <button
                  type="button"
                  class="btn sm toggle"
                  aria-pressed={on}
                  onClick={() => onSelect(on ? null : c.id)}
                >
                  <IconLink size={14} stroke={1.75} aria-hidden="true" />
                  {on ? "Mapping" : "Map hunks"}
                </button>
              </div>
              <div class="seg status" role="group" aria-label={`Status of ${c.id}`} style={{ "--n": STATUSES.length, "--i": idx }}>
                {STATUSES.map(([v, label]) => (
                  <button
                    key={v}
                    type="button"
                    data-v={v}
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
          <div class="section-head">
            <h2>Agent assumptions</h2>
          </div>
          <ul class="assumptions">
            {general.map((a, i) => (
              <li key={i}>
                <IconRobot size={15} stroke={1.75} aria-hidden="true" />
                <span>
                  {a.text}
                  {a.agent && <span class="muted"> {a.agent}</span>}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
