import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { App } from "./app";
import { connect, storeName, storedName, type Session } from "./session";
import type { Analysis } from "./types";
import { NameForm } from "./name-form";

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

  if (error) return <p class="notice" role="alert">{error}</p>;
  if (!name)
    return (
      <NameForm
        onSubmit={(n) => {
          storeName(n);
          setName(n);
        }}
      />
    );
  if (!analysis || !session) return <p class="notice">Loading review...</p>;
  return <App analysis={analysis} session={session} token={token} />;
}

render(<Root />, document.getElementById("root") as HTMLElement);
