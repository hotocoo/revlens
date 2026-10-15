import { IconArrowRight } from "@tabler/icons-preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { PARALLAX_PX } from "./config";
import { shortSha } from "./derive";
import type { Analysis } from "./types";

/** The backdrop drifts opposite the pointer by up to PARALLAX_PX: depth behind the card. */
function useParallax(ref: { current: HTMLElement | null }) {
  useEffect(() => {
    const el = ref.current;
    if (!el || matchMedia("(prefers-reduced-motion: reduce), (pointer: coarse)").matches) return;
    let frame = 0;
    const on = (e: PointerEvent) => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const x = (e.clientX / innerWidth - 0.5) * -2 * PARALLAX_PX;
        const y = (e.clientY / innerHeight - 0.5) * -2 * PARALLAX_PX;
        el.style.transform = `translate3d(${x}px, ${y}px, 0)`;
      });
    };
    addEventListener("pointermove", on, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      removeEventListener("pointermove", on);
    };
  }, [ref]);
}

export function NameForm({ analysis, onSubmit }: { analysis: Analysis | null; onSubmit: (name: string) => void }) {
  const [v, setV] = useState("");
  const backdrop = useRef<HTMLDivElement>(null);
  useParallax(backdrop);
  return (
    <main class="join">
      <div class="join-backdrop" ref={backdrop} aria-hidden="true" />
      <form
        class="join-card"
        onSubmit={(e) => {
          e.preventDefault();
          if (v.trim()) onSubmit(v.trim());
        }}
      >
        <span class="brand" aria-hidden="true">
          <span class="brand-mark" />
          Lens
        </span>
        <h1>Join the review</h1>
        <p class="muted">
          {analysis ? (
            <>
              <span class="mono">
                {shortSha(analysis.range.merge_base)}..{shortSha(analysis.range.head)}
              </span>
              {" "}with {analysis.hunks.length} hunks and {analysis.criteria.length} criteria. Others see your name next to
              what you claim, review and comment on.
            </>
          ) : (
            "Others see your name next to what you claim, review and comment on."
          )}
        </p>
        <label for="name">Your name</label>
        <input
          id="name"
          class="field lg"
          value={v}
          onInput={(e) => setV(e.currentTarget.value)}
          autoFocus
          required
          maxLength={40}
          autoComplete="nickname"
        />
        <button type="submit" class="btn primary lg" disabled={!v.trim()}>
          Join
          <IconArrowRight size={16} stroke={2} aria-hidden="true" />
        </button>
      </form>
    </main>
  );
}
