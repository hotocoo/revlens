import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { App } from "./app";
import { connect, storeName, storedName, type Session } from "./session";
import type { Analysis } from "./types";
import { NameForm } from "./name-form";
import { transition } from "./hooks";

function Loading() {
  return (
    <div class="loading" role="status" aria-label="Loading review">
      {[0, 1, 2].map((i) => (
        <div key={i} class="skeleton" style={{ "--d": i }} />
      ))}
    </div>
  );
}

const params = new URLSearchParams(location.search);
const token = params.get("t") ?? "";

function Root() {
  const [name, setName] = useState(params.get("name") ?? storedName());
  const [analysis, setAnalysis] = useState<Analysis | null>(null);
  const [error, setError] = useState("");
  const [session, setSession] = useState<Session | null>(null);

  useEffect(() => {
    fetch(`/api/analysis?t=${encodeURIComponent(token)}`)
      .then((r) => (r.ok ? (r.json() as Promise<Analysis>) : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then(setAnalysis)
      .catch((e: Error) => setError(`Could not load the analysis (${e.message}). Check the token in the URL.`));
  }, []);

  useEffect(() => {
    if (!analysis || !name) return;
    const s = connect(analysis.room, token, name);
    setSession(s);
    return () => {
      s.provider.destroy();
      s.doc.destroy();
    };
  }, [analysis, name]);

  if (error)
    return (
      <main class="join">
        <div class="join-card" role="alert">
          <h1>Cannot open this review</h1>
          <p class="muted">{error}</p>
        </div>
      </main>
    );
  if (!name)
    return (
      <NameForm
        analysis={analysis}
        onSubmit={(n) => {
          storeName(n);
          transition(() => setName(n));
        }}
      />
    );
  if (!analysis || !session) return <Loading />;
  return <App analysis={analysis} session={session} token={token} />;
}

render(<Root />, document.getElementById("root") as HTMLElement);
