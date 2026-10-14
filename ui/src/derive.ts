import type { Analysis, Hunk } from "./types";
import type { Session } from "./session";

/** Moved, format-only and generated hunks need neither criterion nor reviewer (mirrors conformance.rs). */
export function isDemoted(h: Hunk): boolean {
  return h.kind.type === "Moved" || h.kind.type === "FormatOnly" || h.signals.some((s) => s.rule.type === "Generated");
}

export type Derived = {
  mappedHunks: Set<string>;
  hunksByCrit: Map<string, Set<string>>;
  critsByHunk: Map<string, string[]>;
  metCount: number;
  reviewedCount: number;
  scopeCreep: number;
  missingWork: string[];
};

export function derive(a: Analysis, s: Session): Derived {
  const hunkIds = new Set(a.hunks.map((h) => h.id));
  const critIds = new Set(a.criteria.map((c) => c.id));
  const hunksByCrit = new Map<string, Set<string>>();
  const critsByHunk = new Map<string, string[]>();
  for (const key of s.mappings.keys()) {
    const [crit = "", hunk = ""] = key.split("|");
    if (!hunkIds.has(hunk) || !critIds.has(crit)) continue;
    (hunksByCrit.get(crit) ?? hunksByCrit.set(crit, new Set()).get(crit))?.add(hunk);
    (critsByHunk.get(hunk) ?? critsByHunk.set(hunk, []).get(hunk))?.push(crit);
  }
  const mappedHunks = new Set(critsByHunk.keys());
  return {
    mappedHunks,
    hunksByCrit,
    critsByHunk,
    metCount: a.criteria.filter((c) => s.status.get(c.id) === "Met").length,
    reviewedCount: a.hunks.filter((h) => s.reviewed.has(h.id)).length,
    scopeCreep: a.hunks.filter((h) => !isDemoted(h) && !mappedHunks.has(h.id)).length,
    missingWork: a.criteria
      .filter((c) => !hunksByCrit.has(c.id) && s.status.get(c.id) !== "NotApplicable")
      .map((c) => c.id),
  };
}

export const shortSha = (s: string) => s.slice(0, 8);
