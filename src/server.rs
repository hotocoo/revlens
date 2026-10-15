//! Local web server: static UI, token-guarded JSON/export API and the y-websocket endpoint.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::broadcast::error::RecvError;

use crate::config::Config;
use crate::live::{Session, session_id};
use crate::model::Analysis;
use crate::{conformance, time, token};

const INDEX_HTML: &[u8] = include_bytes!("../ui/dist/index.html");
const APP_JS: &[u8] = include_bytes!("../ui/dist/app.js");
const APP_CSS: &[u8] = include_bytes!("../ui/dist/app.css");
/// Geist and Geist Mono (SIL OFL 1.1, see ui/src/fonts/OFL.txt), served locally: no CDN.
const FONT_SANS: &[u8] = include_bytes!("../ui/dist/geist.woff2");
const FONT_MONO: &[u8] = include_bytes!("../ui/dist/geist-mono.woff2");

const CSP: &str = "default-src 'self'; connect-src 'self' ws: wss:; \
                   style-src 'self' 'unsafe-inline'; font-src 'self'; frame-ancestors 'none'";

#[derive(Clone)]
pub struct AppState {
    pub analysis: Arc<Analysis>,
    pub session: Arc<Session>,
    pub token: Arc<str>,
    /// The only WebSocket room: the session id of the analyzed range.
    pub room: Arc<str>,
    pub max_ws_message_bytes: usize,
    pub ws_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(analysis: Analysis, session: Arc<Session>, token: String, cfg: &Config) -> Self {
        session.set_max_doc_bytes(cfg.server.max_doc_bytes);
        let room = session_id(&analysis.range.merge_base, &analysis.range.head);
        Self {
            analysis: Arc::new(analysis),
            session,
            token: token.into(),
            room: room.into(),
            max_ws_message_bytes: cfg.server.max_ws_message_bytes,
            ws_slots: Arc::new(Semaphore::new(cfg.server.max_ws_connections)),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let guarded = Router::new()
        .route("/api/analysis", get(analysis))
        .route("/api/state", get(session_state))
        .route("/api/export.md", get(export_md))
        .route("/api/export.json", get(export_json))
        .route("/ws/{room}", get(ws))
        .layer(from_fn_with_state(state.clone(), guard));
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", INDEX_HTML, true) }),
        )
        .route(
            "/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", APP_JS, false) }),
        )
        .route(
            "/app.css",
            get(|| async { asset("text/css; charset=utf-8", APP_CSS, false) }),
        )
        .route(
            "/geist.woff2",
            get(|| async { asset("font/woff2", FONT_SANS, false) }),
        )
        .route(
            "/geist-mono.woff2",
            get(|| async { asset("font/woff2", FONT_MONO, false) }),
        )
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .merge(guarded)
        .with_state(state)
}

/// Bind `host:port` (port 0 picks a free one) and report the actual address.
pub async fn bind(host: &str, port: u16) -> Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind((host, port))
        .await
        .with_context(|| format!("binding {host}:{port}"))?;
    let addr = listener.local_addr().context("reading bound address")?;
    Ok((listener, addr))
}

/// Serve until the future is dropped. Debounced persistence is the caller's job.
pub async fn run(listener: TcpListener, state: AppState) -> Result<()> {
    axum::serve(listener, router(state))
        .await
        .context("serving HTTP")
}

pub async fn serve(
    analysis: Analysis,
    session_path: PathBuf,
    cfg: &Config,
    share: bool,
    port: Option<u16>,
    on_ready: impl FnOnce(&str),
) -> Result<()> {
    let session = Arc::new(Session::open(&session_path)?);
    let token = token::generate()?;
    let state = AppState::new(analysis, session.clone(), token.clone(), cfg);
    let host = if share {
        &cfg.server.share_bind
    } else {
        &cfg.server.bind
    };
    let (listener, addr) = bind(host, port.unwrap_or(cfg.server.port)).await?;
    let local_host = if addr.ip().is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        addr.ip().to_string()
    };
    let url = format!("http://{local_host}:{}/?t={token}", addr.port());
    println!("Lens review: {url}");
    on_ready(&url);
    let saver = tokio::spawn(
        session
            .clone()
            .persist_loop(Duration::from_millis(cfg.server.persist_debounce_ms)),
    );
    // Dropping the server future on Ctrl-C also drops open sockets; the session is saved below.
    let result = tokio::select! {
        r = run(listener, state) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
    };
    saver.abort();
    session.persist()?;
    result
}

fn asset(content_type: &'static str, body: &'static [u8], html: bool) -> Response {
    let mut r = Response::new(Body::from(body));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if html {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
        h.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    r
}

/// Cross-site guard, then token check, for every non-static route.
async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if !origin_matches_host(req.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authorized(&st.token, &req) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

/// A request with an `Origin` must come from the page we serve: its authority has to equal `Host`
/// (blocks cross-site pages and DNS rebinding). No `Origin` (curl, CLI) is fine.
fn origin_matches_host(h: &HeaderMap) -> bool {
    let Some(origin) = h.get(header::ORIGIN) else {
        return true;
    };
    let (Some(origin), Some(host)) = (
        origin.to_str().ok(),
        h.get(header::HOST).and_then(|v| v.to_str().ok()),
    ) else {
        return false;
    };
    let authority = origin.split_once("://").map_or("", |(_, rest)| rest);
    authority.eq_ignore_ascii_case(host)
}

fn authorized(expected: &str, req: &Request) -> bool {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let query = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("t=")));
    [bearer, query]
        .into_iter()
        .flatten()
        .any(|given| token::verify(expected, given))
}

fn internal(e: anyhow::Error) -> StatusCode {
    eprintln!("error: {e:#}");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// The analysis plus the websocket room the UI must join.
#[derive(serde::Serialize)]
struct AnalysisView {
    room: Arc<str>,
    #[serde(flatten)]
    analysis: Arc<Analysis>,
}

async fn analysis(State(st): State<AppState>) -> Json<AnalysisView> {
    Json(AnalysisView {
        room: st.room,
        analysis: st.analysis,
    })
}

async fn session_state(State(st): State<AppState>) -> Result<Response, StatusCode> {
    let s = st.session.snapshot().map_err(internal)?;
    Ok(Json(s).into_response())
}

fn record(st: &AppState) -> Result<crate::model::ConformanceRecord, StatusCode> {
    let state = st.session.snapshot().map_err(internal)?;
    Ok(conformance::build(
        &st.analysis,
        &state,
        time::now_rfc3339(),
    ))
}

async fn export_md(State(st): State<AppState>) -> Result<Response, StatusCode> {
    let body = conformance::render_markdown(&record(&st)?);
    Ok((
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        body,
    )
        .into_response())
}

async fn export_json(State(st): State<AppState>) -> Result<Response, StatusCode> {
    let body = conformance::render_json(&record(&st)?).map_err(internal)?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

async fn ws(
    State(st): State<AppState>,
    Path(room): Path<String>,
    up: WebSocketUpgrade,
) -> Response {
    if room != *st.room {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(slot) = st.ws_slots.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    up.max_message_size(st.max_ws_message_bytes)
        .max_frame_size(st.max_ws_message_bytes)
        .on_upgrade(move |socket| async move {
            connection(socket, st.session).await;
            drop(slot);
        })
}

async fn connection(mut socket: WebSocket, session: Arc<Session>) {
    // Subscribe before greeting so nothing committed in between is missed.
    let mut updates = session.subscribe();
    let mut clients = HashSet::new();
    let greeting = match session.greet() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e:#}");
            return;
        }
    };
    let mut outgoing = greeting;
    loop {
        for msg in outgoing.drain(..) {
            if socket.send(WsMessage::Binary(msg.into())).await.is_err() {
                return finish(&session, &clients);
            }
        }
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(WsMessage::Binary(bytes))) => match session.handle_from(&bytes, &mut clients) {
                    Ok(replies) => outgoing = replies,
                    Err(e) => {
                        eprintln!("ws: dropping connection: {e:#}");
                        break;
                    }
                },
                Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            update = updates.recv() => match update {
                Ok(bytes) => outgoing = vec![bytes],
                // Too slow: close; the client reconnects and resyncs from scratch.
                Err(RecvError::Lagged(_)) | Err(RecvError::Closed) => break,
            },
        }
    }
    finish(&session, &clients);
}

fn finish(session: &Session, clients: &HashSet<yrs::ClientID>) {
    if let Err(e) = session.disconnect(clients) {
        eprintln!("error: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).expect("header"));
        }
        h
    }

    #[test]
    fn origin_guard() {
        assert!(origin_matches_host(&headers(&[])));
        assert!(origin_matches_host(&headers(&[("host", "127.0.0.1:9")])));
        assert!(origin_matches_host(&headers(&[
            ("origin", "http://127.0.0.1:9"),
            ("host", "127.0.0.1:9")
        ])));
        assert!(!origin_matches_host(&headers(&[
            ("origin", "http://evil.example"),
            ("host", "127.0.0.1:9")
        ])));
        assert!(!origin_matches_host(&headers(&[("origin", "http://a")])));
        assert!(!origin_matches_host(&headers(&[
            ("origin", "null"),
            ("host", "a")
        ])));
    }

    #[test]
    fn token_from_query_or_bearer() {
        let req = |uri: &str, auth: Option<&str>| {
            let mut b = Request::builder().uri(uri);
            if let Some(a) = auth {
                b = b.header("authorization", a);
            }
            b.body(Body::empty()).expect("request")
        };
        assert!(authorized("tok", &req("/x?t=tok", None)));
        assert!(authorized("tok", &req("/x?a=1&t=tok", None)));
        assert!(authorized("tok", &req("/x", Some("Bearer tok"))));
        assert!(!authorized("tok", &req("/x", None)));
        assert!(!authorized("tok", &req("/x?t=nope", None)));
        assert!(!authorized("tok", &req("/x", Some("Bearer nope"))));
        assert!(!authorized("tok", &req("/x", Some("tok"))));
    }
}
