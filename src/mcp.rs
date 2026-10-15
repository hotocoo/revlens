//! `revlens mcp`: a Model Context Protocol server on stdio (newline-delimited JSON-RPC 2.0) with
//! one tool, `record_assumption`, so coding agents can append to `.revlens/assumptions.jsonl`
//! while they work. Everything an agent sends is validated before it touches the file.

use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::model::Assumption;
use crate::time;

/// Newest protocol revision this server speaks; older client revisions are echoed back.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// Limits on agent input, from `[mcp]` in the config.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_text_chars: usize,
    pub max_agent_chars: usize,
}

pub struct Server {
    root: PathBuf,
    file: PathBuf,
    limits: Limits,
}

impl Server {
    /// `file` is where assumptions are appended; `root` bounds the `path` agents may cite.
    pub fn new(root: &Path, file: PathBuf, limits: Limits) -> Self {
        Self {
            root: root.to_path_buf(),
            file,
            limits,
        }
    }

    /// Serve until stdin closes. Each line is one JSON-RPC message; each response is one line.
    pub fn serve(&self, input: impl BufRead, mut output: impl Write) -> Result<()> {
        for line in input.lines() {
            let line = line.context("reading stdin")?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(&line) {
                writeln!(output, "{reply}").context("writing stdout")?;
                output.flush().context("flushing stdout")?;
            }
        }
        Ok(())
    }

    /// The response to one message, or `None` for a notification.
    pub fn handle_line(&self, line: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str);
        let (Some(id), Some(method)) = (id, method) else {
            // Notifications (no id) and responses to us need no reply.
            return None;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => result(id, self.initialize(&params)),
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({ "tools": [tool_definition(&self.limits)] })),
            "tools/call" => match self.call(&params) {
                Ok(v) => result(id, v),
                Err((code, msg)) => error(id, code, &msg),
            },
            _ => error(id, -32601, &format!("method not found: {method}")),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let asked = params.get("protocolVersion").and_then(Value::as_str);
        let version = asked
            .filter(|v| SUPPORTED.contains(v))
            .unwrap_or(PROTOCOL_VERSION);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "revlens", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Call record_assumption whenever you assume something the task did not state, so reviewers see it next to the code.",
        })
    }

    /// Protocol errors are `Err`; a tool that rejects its input is an `isError` result.
    fn call(&self, params: &Value) -> std::result::Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str);
        if name != Some("record_assumption") {
            return Err((
                -32602,
                format!("unknown tool: {}", name.unwrap_or("(none)")),
            ));
        }
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        Ok(match self.record(&args) {
            Ok(a) => tool_text(
                &format!(
                    "Recorded assumption{}.",
                    a.path
                        .as_deref()
                        .map(|p| format!(
                            " for {p}{}",
                            a.line.map(|l| format!(":{l}")).unwrap_or_default()
                        ))
                        .unwrap_or_default()
                ),
                false,
            ),
            Err(e) => tool_text(&format!("{e:#}"), true),
        })
    }

    fn record(&self, args: &Value) -> Result<Assumption> {
        let a = self.validate(args)?;
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .with_context(|| format!("opening {}", self.file.display()))?;
        let line = serde_json::to_string(&a).context("serializing assumption")?;
        // One write per record keeps concurrent appends from interleaving within a line.
        f.write_all(format!("{line}\n").as_bytes())
            .with_context(|| format!("appending to {}", self.file.display()))?;
        Ok(a)
    }

    fn validate(&self, args: &Value) -> Result<Assumption> {
        let obj = args.as_object().context("arguments must be an object")?;
        for k in obj.keys() {
            anyhow::ensure!(
                ["text", "agent", "path", "line"].contains(&k.as_str()),
                "unknown argument '{k}'"
            );
        }
        let s = |k: &str| -> Result<Option<String>> {
            match obj.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(v)) => Ok(Some(v.trim().to_string())),
                Some(_) => anyhow::bail!("'{k}' must be a string"),
            }
        };
        let text = s("text")?
            .filter(|t| !t.is_empty())
            .context("'text' is required")?;
        let max = self.limits.max_text_chars;
        anyhow::ensure!(
            text.chars().count() <= max,
            "'text' is longer than {max} characters"
        );
        let agent = s("agent")?.filter(|a| !a.is_empty());
        if let Some(a) = &agent {
            let max = self.limits.max_agent_chars;
            anyhow::ensure!(
                a.chars().count() <= max,
                "'agent' is longer than {max} characters"
            );
        }
        let path = s("path")?.filter(|p| !p.is_empty());
        if let Some(p) = &path {
            anyhow::ensure!(
                is_repo_relative(p),
                "'path' must be relative to the repository root, without '..'"
            );
            anyhow::ensure!(
                self.root.join(p).exists(),
                "'path' {p} does not exist in the repository"
            );
        }
        let line = match obj.get("line") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .context("'line' must be a positive integer")?,
            ),
        };
        anyhow::ensure!(line.is_none() || path.is_some(), "'line' needs 'path'");
        Ok(Assumption {
            text,
            agent,
            path,
            line,
            at: Some(time::now_rfc3339()),
        })
    }
}

fn is_repo_relative(p: &str) -> bool {
    let path = Path::new(p);
    !p.contains('\\')
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn tool_definition(l: &Limits) -> Value {
    json!({
        "name": "record_assumption",
        "title": "Record an assumption",
        "description": "Record something you assumed while changing the code (a requirement you inferred, a default you picked, a case you chose not to handle). Reviewers see it beside the diff in Lens. Point at the file and line it concerns when there is one.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The assumption, one or two sentences.", "maxLength": l.max_text_chars },
                "agent": { "type": "string", "description": "Your name, e.g. claude.", "maxLength": l.max_agent_chars },
                "path": { "type": "string", "description": "Repository-relative file the assumption concerns." },
                "line": { "type": "integer", "minimum": 1, "description": "1-based line in that file." }
            },
            "required": ["text"],
            "additionalProperties": false
        }
    })
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::load_assumptions;

    fn server() -> (tempfile::TempDir, Server) {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join("app")).expect("mkdir");
        std::fs::write(dir.path().join("app/billing.py"), "x\n").expect("write");
        let file = dir.path().join(".revlens/assumptions.jsonl");
        let s = Server::new(
            dir.path(),
            file,
            Limits {
                max_text_chars: 50,
                max_agent_chars: 10,
            },
        );
        (dir, s)
    }

    fn call(s: &Server, args: Value) -> Value {
        let msg = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"record_assumption","arguments":args}});
        s.handle_line(&msg.to_string()).expect("reply")["result"].clone()
    }

    #[test]
    fn handshake_list_and_ping() {
        let (_d, s) = server();
        let init = s
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#)
            .expect("init");
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(init["result"]["serverInfo"]["name"], "revlens");
        let newer = s
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2999-01-01"}}"#)
            .expect("init");
        assert_eq!(newer["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(
            s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        let list = s
            .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#)
            .expect("list");
        assert_eq!(list["result"]["tools"][0]["name"], "record_assumption");
        assert_eq!(
            list["result"]["tools"][0]["inputSchema"]["required"],
            json!(["text"])
        );
        let ping = s
            .handle_line(r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#)
            .expect("ping");
        assert_eq!(ping["id"], "p");
        let bad = s
            .handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#)
            .expect("err");
        assert_eq!(bad["error"]["code"], -32601);
        let garbage = s.handle_line("{").expect("parse error");
        assert_eq!(garbage["error"]["code"], -32700);
    }

    #[test]
    fn records_valid_assumptions_readable_by_lens() {
        let (d, s) = server();
        let r = call(
            &s,
            json!({"text":"  Fee is flat  ","agent":"claude","path":"app/billing.py","line":3}),
        );
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(
            r["content"][0]["text"],
            "Recorded assumption for app/billing.py:3."
        );
        call(&s, json!({"text":"General note"}));
        let got = load_assumptions(&d.path().join(".revlens/assumptions.jsonl")).expect("load");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].text, "Fee is flat");
        assert_eq!(got[0].line, Some(3));
        assert!(got[0].at.as_deref().is_some_and(|t| t.ends_with('Z')));
        assert_eq!(got[1].path, None);
    }

    #[test]
    fn rejects_bad_input_without_writing() {
        let (d, s) = server();
        for (args, why) in [
            (json!({}), "'text' is required"),
            (json!({"text":"   "}), "'text' is required"),
            (json!({"text": 5}), "must be a string"),
            (json!({"text":"x".repeat(51)}), "longer than 50"),
            (json!({"text":"a","agent":"x".repeat(11)}), "longer than 10"),
            (json!({"text":"a","path":"../etc/passwd"}), "without '..'"),
            (json!({"text":"a","path":"/etc/passwd"}), "without '..'"),
            (
                json!({"text":"a","path":"app/missing.py"}),
                "does not exist",
            ),
            (
                json!({"text":"a","path":"app/billing.py","line":0}),
                "positive integer",
            ),
            (json!({"text":"a","line":2}), "'line' needs 'path'"),
            (json!({"text":"a","extra":1}), "unknown argument"),
            (json!("text"), "must be an object"),
        ] {
            let r = call(&s, args.clone());
            assert_eq!(r["isError"], true, "{args}");
            let msg = r["content"][0]["text"].as_str().unwrap_or_default();
            assert!(msg.contains(why), "{args}: {msg}");
        }
        assert!(!d.path().join(".revlens/assumptions.jsonl").exists());
        let unknown = s
            .handle_line(r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"rm"}}"#)
            .expect("err");
        assert_eq!(unknown["error"]["code"], -32602);
    }

    #[test]
    fn serve_answers_line_by_line() {
        let (_d, s) = server();
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
        let mut out = Vec::new();
        s.serve(input.as_bytes(), &mut out).expect("serve");
        let lines: Vec<Value> = String::from_utf8(out)
            .expect("utf8")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["id"], 2);
    }
}
