import { IconArrowRight } from "@tabler/icons-preact";
import { useState } from "preact/hooks";
import { shortSha } from "./derive";
import type { Analysis } from "./types";

export function NameForm({ analysis, onSubmit }: { analysis: Analysis | null; onSubmit: (name: string) => void }) {
  const [v, setV] = useState("");
  return (
    <main class="join">
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
