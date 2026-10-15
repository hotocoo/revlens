import { useState } from "preact/hooks";
import { MAX_COMMENT_CHARS } from "./config";
import { addComment, toggleResolved, type Session } from "./session";
import type { Thread } from "./types";

function Composer({ label, onSend }: { label: string; onSend: (text: string) => void }) {
  const [text, setText] = useState("");
  const send = (e: Event) => {
    e.preventDefault();
    onSend(text);
    setText("");
  };
  return (
    <form class="composer" onSubmit={send}>
      <label>
        <span class="sr-only">{label}</span>
        <textarea
          rows={2}
          maxLength={MAX_COMMENT_CHARS}
          placeholder={label}
          value={text}
          onInput={(e) => setText(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) send(e);
          }}
        />
      </label>
      <button type="submit" disabled={!text.trim()}>Send</button>
    </form>
  );
}

/** Comment threads of one hunk, plus a composer for a new thread on `line` (or the whole hunk). */
export function Threads({
  session: s,
  hunk,
  threads,
  line,
  lines,
  onLine,
}: {
  session: Session;
  hunk: string;
  threads: Thread[];
  line: number | null;
  /** First and last new-side line a thread may point at. */
  lines: [number, number];
  onLine: (line: number | null) => void;
}) {
  const [showResolved, setShowResolved] = useState(false);
  const resolvedCount = threads.filter((t) => t.resolvedBy).length;
  const shown = threads.filter((t) => showResolved || !t.resolvedBy);
  return (
    <section class="threads" aria-label="Comments">
      {shown.map((t) => (
        <div key={t.id} class={`thread${t.resolvedBy ? " resolved" : ""}`}>
          <p class="comment">
            <b>{t.root.user}</b>
            {t.root.line != null && <span class="muted mono"> line {t.root.line}</span>}: {t.root.text}
          </p>
          {t.replies.map(([id, c]) => (
            <p key={id} class="comment reply">
              <b>{c.user}</b>: {c.text}
            </p>
          ))}
          <div class="thread-actions">
            <button type="button" aria-pressed={!!t.resolvedBy} onClick={() => toggleResolved(s, t.id)}>
              {t.resolvedBy ? "Reopen" : "Resolve"}
            </button>
            {t.resolvedBy && <span class="label">Resolved by {t.resolvedBy}</span>}
          </div>
          {!t.resolvedBy && <Composer label="Reply" onSend={(text) => addComment(s, hunk, t.root.line, t.id, text)} />}
        </div>
      ))}
      {resolvedCount > 0 && (
        <button type="button" class="link" onClick={() => setShowResolved(!showResolved)}>
          {showResolved ? "Hide" : "Show"} {resolvedCount} resolved
        </button>
      )}
      <div class="new-thread">
        <p class="muted line-pick">
          <label>
            Line{" "}
            <input
              type="number"
              min={lines[0]}
              max={lines[1]}
              value={line ?? ""}
              placeholder="all"
              onInput={(e) => {
                const n = e.currentTarget.valueAsNumber;
                onLine(Number.isInteger(n) && n >= lines[0] && n <= lines[1] ? n : null);
              }}
            />
          </label>
          {line != null && (
            <button type="button" class="link" onClick={() => onLine(null)}>Comment on the whole hunk</button>
          )}
        </p>
        <Composer
          label={line != null ? `Comment on line ${line}` : "Comment on this hunk"}
          onSend={(text) => addComment(s, hunk, line, null, text)}
        />
      </div>
    </section>
  );
}
