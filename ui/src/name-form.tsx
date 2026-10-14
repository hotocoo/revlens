import { useState } from "preact/hooks";

export function NameForm({ onSubmit }: { onSubmit: (name: string) => void }) {
  const [v, setV] = useState("");
  return (
    <form
      class="name-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (v.trim()) onSubmit(v.trim());
      }}
    >
      <h1>Join the review</h1>
      <label for="name">Your name</label>
      <input id="name" value={v} onInput={(e) => setV(e.currentTarget.value)} autoFocus required maxLength={40} />
      <button type="submit" class="primary">Join</button>
    </form>
  );
}
