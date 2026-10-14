//! End to end: real server on a free port, real WebSocket clients speaking y-websocket.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use revlens::config::Config;
use revlens::live::{Session, session_id};
use revlens::model::{Analysis, Claim, CritStatus, Mapping, Range};
use revlens::server::{AppState, bind, run};
use tokio::net::TcpStream as TokioTcp;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use yrs::sync::{Awareness, DefaultProtocol, Message as Y, Protocol, SyncMessage};
use yrs::updates::encoder::Encode;
use yrs::{Doc, Map, ReadTxn, StateVector, Transact};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const LATENCY_BUDGET: Duration = Duration::from_millis(200);

fn analysis() -> Analysis {
    Analysis {
        range: Range {
            base: "main".into(),
            head: "feat".into(),
            merge_base: "aaaa".into(),
        },
        files: vec![],
        hunks: vec![],
        symbols: vec![],
        criteria: vec![],
        assumptions: vec![],
    }
}

struct Server {
    addr: SocketAddr,
    room: String,
    task: JoinHandle<()>,
    saver: JoinHandle<()>,
}

impl Server {
    async fn start(session_file: &Path) -> Server {
        let mut cfg = Config::default();
        cfg.server.persist_debounce_ms = 50;
        Self::start_with(session_file, cfg).await
    }

    async fn start_with(session_file: &Path, cfg: Config) -> Server {
        let session = Arc::new(Session::open(session_file).expect("open session"));
        let state = AppState::new(analysis(), session.clone(), TOKEN.into(), &cfg);
        let (listener, addr) = bind("127.0.0.1", 0).await.expect("bind");
        let saver = tokio::spawn(session.persist_loop(Duration::from_millis(50)));
        let task = tokio::spawn(async move {
            let _ = run(listener, state).await;
        });
        Server {
            addr,
            room: session_id("aaaa", "feat"),
            task,
            saver,
        }
    }

    fn ws_url(&self, room: &str, token: Option<&str>) -> String {
        let t = token.map(|t| format!("?t={t}")).unwrap_or_default();
        format!("ws://{}/ws/{room}{t}", self.addr)
    }

    fn stop(self) {
        self.task.abort();
        self.saver.abort();
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TokioTcp>>;

/// A browser stand-in: its own doc and awareness, y-websocket protocol on the wire.
struct Client {
    aw: Awareness,
    ws: Socket,
    outbox: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Client {
    async fn connect(url: &str) -> Client {
        let (ws, _) = connect_async(url).await.expect("connect");
        Self::over(ws).await
    }

    async fn over(ws: Socket) -> Client {
        let doc = Doc::new();
        let outbox = Arc::new(Mutex::new(Vec::new()));
        let sink = outbox.clone();
        doc.observe_update_v1("outbox", move |_, e| {
            sink.lock().expect("lock").push(e.update.clone());
        })
        .expect("observe");
        let mut c = Client {
            aw: Awareness::new(doc),
            ws,
            outbox,
        };
        let sv = c.aw.doc().transact().state_vector();
        c.send(Y::Sync(SyncMessage::SyncStep1(sv)).encode_v1())
            .await;
        c
    }

    async fn send(&mut self, bytes: Vec<u8>) {
        self.ws
            .send(Message::Binary(bytes.into()))
            .await
            .expect("send");
    }

    /// Send local doc changes made since the last flush.
    async fn flush(&mut self) {
        let pending: Vec<_> = self.outbox.lock().expect("lock").drain(..).collect();
        for u in pending {
            self.send(Y::Sync(SyncMessage::Update(u)).encode_v1()).await;
        }
    }

    fn set(&self, map: &str, key: &str, value: &str) {
        let m = self.aw.doc().get_or_insert_map(map);
        m.insert(&mut self.aw.doc().transact_mut(), key, value);
    }

    fn get(&self, map: &str, key: &str) -> Option<String> {
        let m = self.aw.doc().get_or_insert_map(map);
        m.get(&self.aw.doc().transact(), key)?.cast::<String>().ok()
    }

    /// Process incoming messages until `done` holds or `limit` passes.
    async fn pump_until(&mut self, limit: Duration, done: impl Fn(&Client) -> bool) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if done(self) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(Some(Ok(Message::Binary(b)))) = tokio::time::timeout(left, self.ws.next()).await
            else {
                return done(self);
            };
            let replies = DefaultProtocol.handle(&mut self.aw, &b).expect("protocol");
            // Changes that came from the server must not be echoed back as local edits.
            self.outbox.lock().expect("lock").clear();
            for r in replies {
                self.send(r.encode_v1()).await;
            }
        }
    }
}

fn http_get(addr: SocketAddr, path: &str, extra: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("tcp");
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{extra}\r\n"
    )
    .expect("write");
    let mut raw = String::new();
    s.read_to_string(&mut raw).expect("read");
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .expect("status");
    (status, raw)
}

fn ws_status(err: WsError) -> u16 {
    match err {
        WsError::Http(r) => r.status().as_u16(),
        other => panic!("expected an HTTP rejection, got {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_converge_and_state_survives_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("sessions").join("s.bin");
    let server = Server::start(&file).await;
    let url = server.ws_url(&server.room, Some(TOKEN));
    let mut a = Client::connect(&url).await;
    let mut b = Client::connect(&url).await;
    // Let both finish the initial handshake.
    a.pump_until(Duration::from_millis(100), |_| false).await;
    b.pump_until(Duration::from_millis(100), |_| false).await;

    // A: status + mapping. B must see both well under the latency budget.
    let sent = Instant::now();
    a.set("status", "c1", "Met");
    a.set("mappings", "c1|h_1", "ann");
    a.flush().await;
    let seen = b
        .pump_until(Duration::from_secs(2), |c| {
            c.get("status", "c1").as_deref() == Some("Met")
                && c.get("mappings", "c1|h_1").as_deref() == Some("ann")
        })
        .await;
    let latency = sent.elapsed();
    assert!(seen, "B did not converge within 2s");
    assert!(latency < LATENCY_BUDGET, "sync took {latency:?}");

    // B claims a hunk; A sees it.
    b.set("claims", "h_1", r#"{"user":"bob","at_ms":7}"#);
    b.flush().await;
    assert!(
        a.pump_until(Duration::from_secs(2), |c| c.get("claims", "h_1").is_some())
            .await
    );

    // Presence: A announces itself, B sees it; when A leaves, B sees it removed.
    a.aw.set_local_state_raw(r#"{"user":{"name":"ann"}}"#);
    let update = a.aw.update().expect("awareness update");
    a.send(Y::Awareness(update).encode_v1()).await;
    let a_id = a.aw.client_id();
    assert!(
        b.pump_until(Duration::from_secs(2), |c| c
            .aw
            .state::<serde_json::Value>(a_id)
            .is_some())
            .await
    );
    a.ws.close(None).await.expect("close");
    assert!(
        b.pump_until(Duration::from_secs(2), |c| c
            .aw
            .state::<serde_json::Value>(a_id)
            .is_none())
            .await,
        "presence not removed after disconnect"
    );

    // Debounced persistence, then restart from the same file.
    for _ in 0..100 {
        if file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    server.stop();
    let restarted = Session::open(&file)
        .expect("reopen")
        .snapshot()
        .expect("snapshot");
    assert_eq!(restarted.status["c1"], CritStatus::Met);
    assert_eq!(
        restarted.mappings,
        vec![Mapping {
            criterion: "c1".into(),
            hunk: "h_1".into(),
            by: "ann".into()
        }]
    );
    assert_eq!(
        restarted.claims["h_1"],
        Claim {
            user: "bob".into(),
            at_ms: 7
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn late_joiner_receives_existing_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = Server::start(&dir.path().join("s.bin")).await;
    let url = server.ws_url(&server.room, Some(TOKEN));
    let mut a = Client::connect(&url).await;
    a.pump_until(Duration::from_millis(100), |_| false).await;
    a.set("reviewed", "h_9", "ann");
    a.flush().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut late = Client::connect(&url).await;
    assert!(
        late.pump_until(Duration::from_secs(2), |c| c
            .get("reviewed", "h_9")
            .is_some())
            .await
    );
    server.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_room_and_origin_are_enforced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = Server::start(&dir.path().join("s.bin")).await;
    let addr = server.addr;

    // HTTP API
    assert_eq!(http_get(addr, "/api/analysis", "").0, 401);
    assert_eq!(http_get(addr, "/api/analysis?t=wrong", "").0, 401);
    assert_eq!(http_get(addr, "/api/state", "").0, 401);
    assert_eq!(http_get(addr, "/api/export.md", "").0, 401);
    let (ok, body) = http_get(addr, &format!("/api/analysis?t={TOKEN}"), "");
    assert_eq!(ok, 200);
    assert!(body.contains("\"merge_base\":\"aaaa\""));
    let bearer = format!("Authorization: Bearer {TOKEN}\r\n");
    assert_eq!(http_get(addr, "/api/state", &bearer).0, 200);
    let (md, _) = http_get(addr, &format!("/api/export.md?t={TOKEN}"), "");
    assert_eq!(md, 200);
    let (js, body) = http_get(addr, &format!("/api/export.json?t={TOKEN}"), "");
    assert_eq!(js, 200);
    assert!(body.contains("\"schema\""));
    let cross = "Origin: http://evil.example\r\n".to_string();
    assert_eq!(
        http_get(addr, &format!("/api/analysis?t={TOKEN}"), &cross).0,
        403
    );

    // Static assets need no token and carry security headers on HTML.
    let (st, html) = http_get(addr, "/", "");
    assert_eq!(st, 200);
    let lower = html.to_ascii_lowercase();
    assert!(
        lower.contains("content-security-policy: default-src 'self'; connect-src 'self' ws: wss:")
    );
    assert!(lower.contains("x-content-type-options: nosniff"));
    assert!(lower.contains("referrer-policy: no-referrer"));
    assert!(lower.contains("x-frame-options: deny"));
    assert_eq!(http_get(addr, "/app.js", "").0, 200);
    assert_eq!(http_get(addr, "/app.css", "").0, 200);

    // WebSocket
    let no_token = connect_async(server.ws_url(&server.room, None))
        .await
        .expect_err("no token");
    assert_eq!(ws_status(no_token), 401);
    let bad = connect_async(server.ws_url(&server.room, Some("nope")))
        .await
        .expect_err("bad token");
    assert_eq!(ws_status(bad), 401);
    let wrong_room = connect_async(server.ws_url("s_000000000000", Some(TOKEN)))
        .await
        .expect_err("wrong room");
    assert_eq!(ws_status(wrong_room), 404);

    let mut evil = server
        .ws_url(&server.room, Some(TOKEN))
        .into_client_request()
        .expect("request");
    evil.headers_mut()
        .insert("Origin", "http://evil.example".parse().expect("header"));
    assert_eq!(
        ws_status(connect_async(evil).await.expect_err("cross origin")),
        403
    );

    // Same-origin browsers send Origin equal to Host and are accepted.
    let mut same = server
        .ws_url(&server.room, Some(TOKEN))
        .into_client_request()
        .expect("request");
    same.headers_mut()
        .insert("Origin", format!("http://{addr}").parse().expect("header"));
    connect_async(same).await.expect("same origin accepted");
    server.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_frames_close_only_that_connection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = Server::start(&dir.path().join("s.bin")).await;
    let url = server.ws_url(&server.room, Some(TOKEN));
    let (mut bad, _) = connect_async(&url).await.expect("connect");
    bad.send(Message::Binary(vec![9, 1, 0].into()))
        .await
        .expect("send");
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(m)) = bad.next().await {
            if matches!(m, Message::Close(_)) {
                return true;
            }
        }
        true
    })
    .await
    .expect("connection closed");
    assert!(closed);

    // An oversized message (default limit 1 MiB) is refused too.
    let (mut big, _) = connect_async(&url).await.expect("connect");
    let _ = big.send(Message::Binary(vec![0u8; 2 << 20].into())).await;
    let ended = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match big.next().await {
                Some(Ok(Message::Binary(_))) => continue,
                _ => return,
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "oversized message did not close the socket");

    // The server still serves well-formed clients.
    let mut ok = Client::connect(&url).await;
    ok.set("status", "c2", "NotMet");
    ok.flush().await;
    let mut peer = Client::connect(&url).await;
    assert!(
        peer.pump_until(Duration::from_secs(2), |c| c.get("status", "c2").is_some())
            .await
    );
    let _ = StateVector::default();
    server.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_cap_refuses_extra_sockets_and_frees_slots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = Config::default();
    cfg.server.max_ws_connections = 1;
    let server = Server::start_with(&dir.path().join("s.bin"), cfg).await;
    let url = server.ws_url(&server.room, Some(TOKEN));
    let (mut first, _) = connect_async(&url).await.expect("first connects");
    assert!(connect_async(&url).await.is_err(), "second must be refused");
    first.close(None).await.expect("close");
    drop(first);
    let mut freed = false;
    for _ in 0..50 {
        if connect_async(&url).await.is_ok() {
            freed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(freed, "slot not released after close");
    server.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_doc_drops_the_writer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = Config::default();
    cfg.server.max_doc_bytes = 100;
    let server = Server::start_with(&dir.path().join("s.bin"), cfg).await;
    let url = server.ws_url(&server.room, Some(TOKEN));
    let mut c = Client::connect(&url).await;
    c.set("status", "c1", &"x".repeat(500));
    c.flush().await;
    c.set("status", "c2", "Met");
    c.flush().await;
    let mut peer = Client::connect(&url).await;
    assert!(
        !peer
            .pump_until(Duration::from_secs(1), |p| p.get("status", "c2").is_some())
            .await,
        "update past the limit must not be applied"
    );
    server.stop();
}
