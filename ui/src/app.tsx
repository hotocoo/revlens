import { IconArrowsShuffle, IconSearch, IconX } from "@tabler/icons-preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { OPEN_BY_DEFAULT, WIDE_QUERY } from "./config";
import { Criteria } from "./criteria";
import { derive, isDemoted } from "./derive";
import { HunkCard } from "./hunk-card";
import { useKeys, useMedia, useReveal } from "./hooks";
import { peers, type Session } from "./session";
import { Shortcuts } from "./shortcuts";
import { TopBar } from "./topbar";
import type { Analysis } from "./types";

/** Re-render whenever shared state or presence changes. */
function useRevision(s: Session): number {
  const [rev, setRev] = useState(0);
  useEffect(() => {
    const bump = () => setRev((r) => r + 1);
    const maps = [s.status, s.mappings, s.reviewed, s.claims, s.comments, s.resolved];
    maps.forEach((m) => m.observe(bump));
    s.provider.awareness.on("change", bump);
    s.provider.on("status", bump);
    s.provider.on("sync", bump);
    // The provider connects before this effect runs; catch up on anything already received.
    bump();
    return () => {
      maps.forEach((m) => m.unobserve(bump));
      s.provider.awareness.off("change", bump);
      s.provider.off("status", bump);
      s.provider.off("sync", bump);
    };
  }, [s]);
  return rev;
}

type View = "all" | "todo" | "discussed";
const VIEWS: [View, string][] = [
  ["all", "All"],
  ["todo", "To review"],
  ["discussed", "Discussed"],
];

/** Card holding focus, or the first one. */
function currentCard(): HTMLElement | null {
  const active = document.activeElement?.closest<HTMLElement>("[data-hunk]");
  return active ?? document.querySelector<HTMLElement>("[data-hunk]");
}

function focusCard(step: 1 | -1) {
  const cards = [...document.querySelectorAll<HTMLElement>("[data-hunk]")];
  if (!cards.length) return;
  const at = document.activeElement?.closest<HTMLElement>("[data-hunk]");
  const i = at ? cards.indexOf(at) : -1;
  const next = cards[Math.max(0, Math.min(cards.length - 1, i < 0 ? 0 : i + step))];
  const target = next?.querySelector<HTMLElement>("[data-act=toggle]");
  target?.focus();
  target?.scrollIntoView({ block: "nearest", behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
}

const act = (name: string) => () => currentCard()?.querySelector<HTMLElement>(`[data-act=${name}]`)?.click();

export function App({ analysis, session, token }: { analysis: Analysis; session: Session; token: string }) {
  useRevision(session);
  const wide = useMedia(WIDE_QUERY);
  const [selected, setSelected] = useState<string | null>(null);
  const [showDemoted, setShowDemoted] = useState(true);
  const [view, setView] = useState<View>("all");
  const [filter, setFilter] = useState("");
  const drawer = useRef<HTMLDialogElement>(null);
  const help = useRef<HTMLDialogElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLOListElement>(null);

  const d = derive(analysis, session);
  const ps = peers(session);
  const hunks = analysis.hunks;
  const initialOpen = useMemo(
    () => new Set(hunks.filter((h) => !isDemoted(h)).slice(0, OPEN_BY_DEFAULT).map((h) => h.id)),
    [hunks],
  );
  const q = filter.toLowerCase();
  const visible = hunks.filter(
    (h) =>
      (showDemoted || !isDemoted(h)) &&
      h.path.toLowerCase().includes(q) &&
      (view === "all" ||
        (view === "todo" && !isDemoted(h) && !session.reviewed.has(h.id)) ||
        (view === "discussed" && (d.openThreads.get(h.id) ?? 0) > 0)),
  );
  const visibleKey = visible.map((h) => h.id).join();
  useReveal(() => list.current, [visibleKey]);

  const selectCriterion = (id: string | null) => {
    setSelected(id);
    if (id && !wide) drawer.current?.close();
  };
  const crit = analysis.criteria.find((c) => c.id === selected);
  const mappedCount = selected ? (d.hunksByCrit.get(selected)?.size ?? 0) : 0;

  const keys = useMemo(
    () => ({
      j: () => focusCard(1),
      k: () => focusCard(-1),
      o: act("toggle"),
      r: act("review"),
      c: act("claim"),
      m: act("map"),
      "/": () => search.current?.focus(),
      "?": () => help.current?.showModal(),
      Escape: () => setSelected(null),
    }),
    [],
  );
  useKeys(keys);

  const criteria = (
    <Criteria
      analysis={analysis}
      session={session}
      derived={d}
      selected={selected}
      onSelect={selectCriterion}
      general={analysis.assumptions.filter((a) => !a.path)}
    />
  );

  return (
    <div class={`shell${selected ? " mapping" : ""}`}>
      <a class="skip" href="#hunks">Skip to hunks</a>
      <TopBar
        analysis={analysis}
        derived={d}
        peers={ps}
        connected={session.provider.wsconnected}
        token={token}
        onOpenCriteria={wide ? undefined : () => drawer.current?.showModal()}
        onShortcuts={() => help.current?.showModal()}
      />
      <div class="layout">
        {wide ? (
          <aside class="rail" aria-label="Acceptance criteria">
            {criteria}
          </aside>
        ) : (
          <dialog
            ref={drawer}
            class="drawer"
            aria-label="Acceptance criteria"
            onClick={(e) => e.target === drawer.current && drawer.current?.close()}
          >
            <div class="drawer-head">
              <h2>Criteria</h2>
              <button type="button" class="btn ghost icon" onClick={() => drawer.current?.close()} aria-label="Close criteria">
                <IconX size={18} stroke={1.75} />
              </button>
            </div>
            {criteria}
          </dialog>
        )}

        <main class="feed" id="hunks" tabIndex={-1} aria-label="Hunks ranked by risk">
          <div class="toolbar">
            <label class="search">
              <IconSearch size={16} stroke={1.75} aria-hidden="true" />
              <span class="sr-only">Filter by path</span>
              <input
                ref={search}
                class="field"
                type="search"
                placeholder="Filter by path"
                value={filter}
                onInput={(e) => setFilter(e.currentTarget.value)}
              />
              <kbd aria-hidden="true">/</kbd>
            </label>
            <div class="seg views" role="group" aria-label="Show" style={{ "--n": VIEWS.length, "--i": VIEWS.findIndex(([v]) => v === view) }}>
              {VIEWS.map(([v, label]) => (
                <button key={v} type="button" aria-pressed={view === v} onClick={() => setView(v)}>
                  {label}
                </button>
              ))}
            </div>
            <label class="switch">
              <input type="checkbox" checked={showDemoted} onChange={(e) => setShowDemoted(e.currentTarget.checked)} />
              <IconArrowsShuffle size={15} stroke={1.75} aria-hidden="true" />
              Moved and format-only
            </label>
            <span class="count muted num" aria-live="polite">
              {visible.length} of {hunks.length}
            </span>
          </div>

          <ol class="cards" ref={list}>
            {visible.map((h, i) => (
              <HunkCard
                key={h.id}
                index={i}
                hunk={h}
                analysis={analysis}
                session={session}
                peers={ps}
                crits={d.critsByHunk.get(h.id) ?? []}
                selected={selected}
                defaultOpen={initialOpen.has(h.id)}
              />
            ))}
          </ol>
          {visible.length === 0 && (
            <div class="empty">
              <p>No hunks match.</p>
              <button
                type="button"
                class="btn sm"
                onClick={() => {
                  setFilter("");
                  setView("all");
                  setShowDemoted(true);
                }}
              >
                Clear filters
              </button>
            </div>
          )}
        </main>
      </div>

      {crit && (
        <div class="mapbar" role="status">
          <span class="mapbar-id mono">{crit.id}</span>
          <span class="mapbar-text">
            Choose the hunks that implement <b>{crit.text}</b>
          </span>
          <span class="mapbar-count num">{mappedCount} mapped</span>
          <button type="button" class="btn primary sm" onClick={() => setSelected(null)}>
            Done <kbd>Esc</kbd>
          </button>
        </div>
      )}
      <Shortcuts ref={help} />
    </div>
  );
}
