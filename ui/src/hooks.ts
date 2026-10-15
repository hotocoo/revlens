import { useEffect, useState } from "preact/hooks";
import { REVEAL_STAGGER_MAX } from "./config";

export function useMedia(query: string): boolean {
  const [hit, setHit] = useState(() => matchMedia(query).matches);
  useEffect(() => {
    const m = matchMedia(query);
    const on = () => setHit(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, [query]);
  return hit;
}

/**
 * Sets `data-in` on every `.reveal` element under `root` as it scrolls into view. Elements that
 * arrive in the same batch get increasing `--d`, so they stagger (capped at REVEAL_STAGGER_MAX).
 * `deps` re-scans after the list changes.
 */
export function useReveal(root: () => Element | null, deps: unknown[]) {
  useEffect(() => {
    const el = root();
    if (!el) return;
    const io = new IntersectionObserver(
      (entries) => {
        let k = 0;
        for (const e of entries) {
          if (!e.isIntersecting) continue;
          const t = e.target as HTMLElement;
          t.style.setProperty("--d", String(Math.min(k++, REVEAL_STAGGER_MAX)));
          t.dataset.in = ""; // an attribute, not a class: re-renders rewrite `class`
          io.unobserve(t);
        }
      },
      { rootMargin: "0px 0px -6% 0px" },
    );
    el.querySelectorAll(".reveal:not([data-in])").forEach((n) => io.observe(n));
    return () => io.disconnect();
  }, deps);
}

/** Single-key shortcuts; ignored while typing or with a modifier held. */
export function useKeys(map: Record<string, (e: KeyboardEvent) => void>) {
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
      if (t?.closest("dialog[open]") && e.key !== "Escape") return;
      const f = map[e.key];
      if (!f) return;
      e.preventDefault();
      f(e);
    };
    addEventListener("keydown", on);
    return () => removeEventListener("keydown", on);
  }, [map]);
}

/** Run a DOM-changing update as a view transition when the browser supports it. */
export function transition(update: () => void) {
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const doc = document as Document & { startViewTransition?: (cb: () => Promise<void>) => unknown };
  if (!doc.startViewTransition || reduce) return update();
  // Preact renders on the next tick; let the new screen paint before the "after" snapshot.
  doc.startViewTransition(() => {
    update();
    return new Promise<void>((done) => requestAnimationFrame(() => done()));
  });
}
