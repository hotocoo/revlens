import { IconX } from "@tabler/icons-preact";
import { forwardRef } from "preact/compat";

const KEYS: [string, string][] = [
  ["j / k", "Next or previous hunk"],
  ["o", "Expand or collapse the diff"],
  ["r", "Mark reviewed"],
  ["c", "Claim or release"],
  ["m", "Map to the criterion being mapped"],
  ["/", "Filter by path"],
  ["Esc", "Leave map mode, close panels"],
  ["?", "This list"],
];

export const Shortcuts = forwardRef<HTMLDialogElement>(function Shortcuts(_props, ref) {
  return (
    <dialog ref={ref} class="sheet" aria-labelledby="shortcuts-title">
      <div class="sheet-head">
        <h2 id="shortcuts-title">Keyboard</h2>
        <form method="dialog">
          <button class="btn ghost icon" aria-label="Close">
            <IconX size={18} stroke={1.75} />
          </button>
        </form>
      </div>
      <dl class="keys">
        {KEYS.map(([k, what]) => (
          <div key={k}>
            <dt>
              {k.split(" / ").map((x, i) => (
                <span key={x}>
                  {i > 0 && " / "}
                  <kbd>{x}</kbd>
                </span>
              ))}
            </dt>
            <dd>{what}</dd>
          </div>
        ))}
      </dl>
    </dialog>
  );
});
