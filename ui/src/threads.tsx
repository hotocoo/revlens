import { IconCircleCheck, IconSend } from "@tabler/icons-preact";
import { useState } from "preact/hooks";
import { MAX_COMMENT_CHARS } from "./config";
import { addComment, colorFor, toggleResolved, type Session } from "./session";
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
          class="field"
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
      <button type="submit" class="btn primary sm" disabled={!text.trim()}>
        <IconSend size={14} stroke={1.75} aria-hidden="true" />
        Send
      </button>
    </form>
  );
}

const when = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });

function Note({ user, at, line, text, reply }: { user: string; at: number; line: number | null; text: string; reply?: boolean }) {
  return (
    <div class={`note${reply ? " reply" : ""}`}>
      <span class="note-avatar" style={{ "--c": colorFor(user) }} aria-hidden="true">
        {user.slice(0, 1).toUpperCase()}
      </span>
      <div>
        <p class="note-head">
          <b>{user}</b>
          {line != null && <span class="mono muted">line {line}</span>}
          <time class="muted" dateTime={new Date(at).toISOString()}>
            {when.format(at)}
          </time>
        </p>
        <p class="note-text">{text}</p>
      </div>
    </div>
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
          <Note user={t.root.user} at={t.root.at_ms} line={t.root.line} text={t.root.text} />
          {t.replies.map(([id, c]) => (
            <Note key={id} user={c.user} at={c.at_ms} line={null} text={c.text} reply />
          ))}
          <div class="thread-actions">
            <button type="button" class="btn sm ghost" aria-pressed={!!t.resolvedBy} onClick={() => toggleResolved(s, t.id)}>
              <IconCircleCheck size={14} stroke={1.75} aria-hidden="true" />
              {t.resolvedBy ? "Reopen" : "Resolve"}
            </button>
            {t.resolvedBy && <span class="muted">Resolved by {t.resolvedBy}</span>}
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
              class="field"
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
