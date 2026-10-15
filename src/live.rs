//! Live review session: one `yrs::Doc` + `Awareness` per session, speaking the y-websocket wire
//! protocol (message type 0 = sync, 1 = awareness) so the browser can use yjs unchanged.
//!
//! Doc layout (top-level maps, all string values):
//! - `status`:   `critId` -> `"Open" | "Met" | "NotMet" | "NotApplicable"`
//! - `mappings`: `"<critId>|<hunkId>"` -> user who made the mapping
//! - `reviewed`: `hunkId` -> user who marked it reviewed
//! - `claims`:   `hunkId` -> JSON string `{"user":"..","at_ms":123}` (a JSON string, not a nested
//!   map, so claim and re-claim are plain last-writer-wins key sets; the UI writes the same shape)

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::{Notify, broadcast};
use yrs::encoding::read::Cursor;
use yrs::sync::{Awareness, DefaultProtocol, Message, MessageReader, Protocol, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, Map, ReadTxn, StateVector, Transact, Update};

use crate::hash::fnv1a64;
use crate::model::{Claim, CritStatus, Mapping, SessionState};

const STATUS: &str = "status";
const MAPPINGS: &str = "mappings";
const REVIEWED: &str = "reviewed";
const CLAIMS: &str = "claims";
const OBSERVER_KEY: &str = "lens-live";
/// Buffered outgoing messages per slow subscriber before it lags (and must resync on reconnect).
const BROADCAST_CAPACITY: usize = 256;

/// `"s_" + 12 hex` of a stable hash of the compared range.
pub fn session_id(merge_base: &str, head: &str) -> String {
    let h = fnv1a64(format!("{merge_base}\0{head}").as_bytes());
    format!("s_{:012x}", h >> 16)
}

pub struct Session {
    awareness: Mutex<Awareness>,
    doc: Doc,
    tx: broadcast::Sender<Vec<u8>>,
    path: PathBuf,
    dirty: Arc<Notify>,
    /// Running total of update bytes in the doc; an upper-bound proxy for its encoded size.
    size: Arc<AtomicUsize>,
    max_size: AtomicUsize,
}

impl Session {
    /// Open the session at `path`, loading the persisted v1 update if the file exists.
    pub fn open(path: &Path) -> Result<Session> {
        let doc = Doc::new();
        let size = Arc::new(AtomicUsize::new(0));
        if path.exists() {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            size.store(bytes.len(), Ordering::Relaxed);
            let update = Update::decode_v1(&bytes)
                .with_context(|| format!("decoding session file {}", path.display()))?;
            doc.transact_mut()
                .apply_update(update)
                .with_context(|| format!("applying session file {}", path.display()))?;
        }
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let dirty = Arc::new(Notify::new());
        let (obs_tx, obs_dirty, obs_size) = (tx.clone(), dirty.clone(), size.clone());
        doc.observe_update_v1(OBSERVER_KEY, move |_, e| {
            // Every committed change, from any client, fans out to all connections.
            // An Err only means nobody is subscribed.
            let _ = obs_tx.send(Message::Sync(SyncMessage::Update(e.update.clone())).encode_v1());
            obs_size.fetch_add(e.update.len(), Ordering::Relaxed);
            obs_dirty.notify_one();
        })
        .map_err(|_| anyhow!("registering doc observer"))?;
        Ok(Session {
            awareness: Mutex::new(Awareness::new(doc.clone())),
            doc,
            tx,
            path: path.to_path_buf(),
            dirty,
            size,
            max_size: AtomicUsize::new(usize::MAX),
        })
    }

    /// Refuse further client messages once the doc holds more than `bytes` (an update may
    /// overshoot by one message, so the doc stays within `bytes` + the max message size).
    pub fn set_max_doc_bytes(&self, bytes: usize) {
        self.max_size.store(bytes, Ordering::Relaxed);
    }

    fn lock(&self) -> Result<MutexGuard<'_, Awareness>> {
        self.awareness
            .lock()
            .map_err(|_| anyhow!("session lock poisoned"))
    }

    /// Updates and awareness changes made by any connection (including the sender).
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.tx.subscribe()
    }

    /// Messages a new connection must receive first: our state vector and current awareness.
    pub fn greet(&self) -> Result<Vec<Vec<u8>>> {
        let aw = self.lock()?;
        let sv = self.doc.transact().state_vector();
        let presence = aw.update().context("encoding awareness")?;
        Ok(vec![
            Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1(),
            Message::Awareness(presence).encode_v1(),
        ])
    }

    /// Handle one binary message; returns replies for the sender only. Doc changes and awareness
    /// updates reach everyone through [`Session::subscribe`].
    pub fn handle(&self, msg: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.handle_from(msg, &mut HashSet::new())
    }

    /// Like [`Session::handle`], recording which awareness clients this connection introduced so
    /// they can be removed on disconnect.
    pub fn handle_from(&self, msg: &[u8], clients: &mut HashSet<ClientID>) -> Result<Vec<Vec<u8>>> {
        if self.size.load(Ordering::Relaxed) > self.max_size.load(Ordering::Relaxed) {
            bail!("session document exceeds its size limit");
        }
        let mut aw = self.lock()?;
        let mut decoder = DecoderV1::new(Cursor::new(msg));
        let mut replies = Vec::new();
        for m in MessageReader::new(&mut decoder) {
            let m = m.context("decoding sync message")?;
            let presence = match &m {
                Message::Awareness(u) => {
                    clients.extend(u.clients.keys().copied());
                    Some(m.encode_v1())
                }
                _ => None,
            };
            let reply = DefaultProtocol
                .handle_message(&mut aw, m)
                .map_err(|e| anyhow!("sync protocol: {e}"))?;
            if let Some(r) = reply {
                replies.push(r.encode_v1());
            }
            if let Some(p) = presence {
                let _ = self.tx.send(p);
            }
        }
        Ok(replies)
    }

    /// Clear the presence of a closed connection and tell everyone else.
    pub fn disconnect(&self, clients: &HashSet<ClientID>) -> Result<()> {
        if clients.is_empty() {
            return Ok(());
        }
        let mut aw = self.lock()?;
        for c in clients {
            aw.remove_state(*c);
        }
        let update = aw
            .update_with_clients(clients.iter().copied())
            .context("encoding awareness removal")?;
        let _ = self.tx.send(Message::Awareness(update).encode_v1());
        Ok(())
    }

    pub fn snapshot(&self) -> Result<SessionState> {
        let _aw = self.lock()?;
        let (status, mappings, reviewed, claims) = (
            self.doc.get_or_insert_map(STATUS),
            self.doc.get_or_insert_map(MAPPINGS),
            self.doc.get_or_insert_map(REVIEWED),
            self.doc.get_or_insert_map(CLAIMS),
        );
        let txn = self.doc.transact();
        let strings = |m: &yrs::MapRef| -> Vec<(String, String)> {
            m.iter(&txn)
                .filter_map(|(k, v)| Some((k.to_string(), v.cast::<String>().ok()?)))
                .collect()
        };
        let mut state = SessionState::default();
        for (crit, s) in strings(&status) {
            if let Some(s) = parse_status(&s) {
                state.status.insert(crit, s);
            }
        }
        for (key, by) in strings(&mappings) {
            if let Some((criterion, hunk)) = key.split_once('|') {
                state.mappings.push(Mapping {
                    criterion: criterion.into(),
                    hunk: hunk.into(),
                    by,
                });
            }
        }
        state.mappings.sort();
        state.reviewed = strings(&reviewed).into_iter().collect();
        for (hunk, json) in strings(&claims) {
            if let Ok(c) = serde_json::from_str::<Claim>(&json) {
                state.claims.insert(hunk, c);
            }
        }
        Ok(state)
    }

    /// Write `state` into the doc (used to seed a re-anchored session).
    pub fn import(&self, state: &SessionState) {
        apply_state(&self.doc, state);
    }

    /// Atomic write (tmp file + rename) of the full doc state.
    pub fn persist(&self) -> Result<()> {
        let bytes = {
            let _aw = self.lock()?;
            self.doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default())
        };
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))
    }

    /// Persist after `debounce` of quiet following any doc change. Runs until aborted.
    pub async fn persist_loop(self: Arc<Self>, debounce: Duration) {
        loop {
            self.dirty.notified().await;
            while tokio::time::timeout(debounce, self.dirty.notified())
                .await
                .is_ok()
            {}
            if let Err(e) = self.persist() {
                eprintln!("warning: persisting session: {e:#}");
            }
        }
    }
}

fn parse_status(s: &str) -> Option<CritStatus> {
    Some(match s {
        "Open" => CritStatus::Open,
        "Met" => CritStatus::Met,
        "NotMet" => CritStatus::NotMet,
        "NotApplicable" => CritStatus::NotApplicable,
        _ => return None,
    })
}

/// Write a [`SessionState`] into a doc using the layout above (test and import helper).
pub fn apply_state(doc: &Doc, state: &SessionState) {
    let (status, mappings, reviewed, claims) = (
        doc.get_or_insert_map(STATUS),
        doc.get_or_insert_map(MAPPINGS),
        doc.get_or_insert_map(REVIEWED),
        doc.get_or_insert_map(CLAIMS),
    );
    let mut txn = doc.transact_mut();
    for (crit, s) in &state.status {
        status.insert(&mut txn, crit.as_str(), format!("{s:?}"));
    }
    for m in &state.mappings {
        mappings.insert(
            &mut txn,
            format!("{}|{}", m.criterion, m.hunk),
            m.by.as_str(),
        );
    }
    for (hunk, user) in &state.reviewed {
        reviewed.insert(&mut txn, hunk.as_str(), user.as_str());
    }
    for (hunk, claim) in &state.claims {
        if let Ok(json) = serde_json::to_string(claim) {
            claims.insert(&mut txn, hunk.as_str(), json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use yrs::sync::AwarenessUpdate;

    fn sample() -> SessionState {
        SessionState {
            status: BTreeMap::from([
                ("c1".to_string(), CritStatus::Met),
                ("c2".to_string(), CritStatus::NotApplicable),
            ]),
            mappings: vec![Mapping {
                criterion: "c1".into(),
                hunk: "h_abc".into(),
                by: "ann".into(),
            }],
            reviewed: BTreeMap::from([("h_abc".to_string(), "bob".to_string())]),
            claims: BTreeMap::from([(
                "h_def".to_string(),
                Claim {
                    user: "ann".into(),
                    at_ms: 42,
                },
            )]),
        }
    }

    fn open_tmp() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = Session::open(&dir.path().join("sessions").join("s.bin")).expect("open");
        (dir, s)
    }

    #[test]
    fn session_id_is_stable_and_range_sensitive() {
        let a = session_id("aaa", "bbb");
        assert_eq!(a, session_id("aaa", "bbb"));
        assert_ne!(a, session_id("aaa", "bbc"));
        assert_ne!(session_id("ab", "c"), session_id("a", "bc"));
        assert!(a.starts_with("s_") && a.len() == 14);
    }

    #[test]
    fn state_round_trips_through_doc() {
        let (_d, s) = open_tmp();
        apply_state(&s.doc, &sample());
        assert_eq!(s.snapshot().expect("snapshot"), sample());
    }

    #[test]
    fn persist_and_reopen_keeps_state() {
        let (dir, s) = open_tmp();
        apply_state(&s.doc, &sample());
        s.persist().expect("persist");
        assert!(!dir.path().join("sessions").join("s.tmp").exists());
        let again = Session::open(&dir.path().join("sessions").join("s.bin")).expect("reopen");
        assert_eq!(again.snapshot().expect("snapshot"), sample());
    }

    #[test]
    fn corrupt_session_file_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("s.bin");
        std::fs::write(&p, [0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).expect("write");
        assert!(Session::open(&p).is_err());
    }

    #[test]
    fn handle_replies_to_step1_and_applies_updates_with_broadcast() {
        let (_d, s) = open_tmp();
        let mut rx = s.subscribe();
        // A client doc with some state sends its update; the server applies and broadcasts it.
        let client = Doc::new();
        apply_state(&client, &sample());
        let update = client
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let msg = Message::Sync(SyncMessage::Update(update)).encode_v1();
        assert!(s.handle(&msg).expect("update").is_empty());
        assert_eq!(s.snapshot().expect("snapshot"), sample());
        let b = rx.try_recv().expect("broadcast");
        assert_eq!(b[0], 0, "sync message type");
        // Step1 from an empty client yields a step2 reply.
        let step1 = Message::Sync(SyncMessage::SyncStep1(StateVector::default())).encode_v1();
        let replies = s.handle(&step1).expect("step1");
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0][..2], [0, 1], "sync / step2");
    }

    #[test]
    fn greet_sends_step1_then_awareness() {
        let (_d, s) = open_tmp();
        let g = s.greet().expect("greet");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0][..2], [0, 0]);
        assert_eq!(g[1][0], 1);
    }

    #[test]
    fn awareness_is_broadcast_tracked_and_removed_on_disconnect() {
        let (_d, s) = open_tmp();
        let mut rx = s.subscribe();
        let mut remote = Awareness::new(Doc::new());
        remote.set_local_state_raw(r#"{"user":{"name":"ann"}}"#);
        let msg = Message::Awareness(remote.update().expect("update")).encode_v1();
        let mut clients = HashSet::new();
        s.handle_from(&msg, &mut clients).expect("awareness");
        assert_eq!(clients, HashSet::from([remote.client_id()]));
        assert_eq!(rx.try_recv().expect("presence broadcast")[0], 1);

        s.disconnect(&clients).expect("disconnect");
        let gone = rx.try_recv().expect("removal broadcast");
        let mut dec = DecoderV1::new(Cursor::new(&gone));
        let Some(Ok(Message::Awareness(AwarenessUpdate { clients: removed }))) =
            MessageReader::new(&mut dec).next()
        else {
            panic!("expected awareness message");
        };
        assert_eq!(&*removed[&remote.client_id()].json, "null");
        // Removed clients no longer appear in what new connections receive.
        let g = s.greet().expect("greet");
        let mut dec = DecoderV1::new(Cursor::new(&g[1]));
        let Some(Ok(Message::Awareness(u))) = MessageReader::new(&mut dec).next() else {
            panic!("expected awareness");
        };
        assert!(u.clients.is_empty());
        s.disconnect(&HashSet::new())
            .expect("empty disconnect is a no-op");
    }

    #[test]
    fn garbage_message_is_an_error_not_a_panic() {
        let (_d, s) = open_tmp();
        assert!(s.handle(&[9, 1, 0]).is_err());
        assert!(s.handle(&[2, 0, 5, b'n', b'o', b'p', b'e', b'!']).is_err());
    }

    #[tokio::test]
    async fn persist_loop_writes_after_quiet_period() {
        let (dir, s) = open_tmp();
        let s = Arc::new(s);
        let task = tokio::spawn(s.clone().persist_loop(Duration::from_millis(30)));
        apply_state(&s.doc, &sample());
        let path = dir.path().join("sessions").join("s.bin");
        for _ in 0..100 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        task.abort();
        let again = Session::open(&path).expect("reopen");
        assert_eq!(again.snapshot().expect("snapshot"), sample());
    }

    #[test]
    fn oversized_doc_rejects_further_messages() {
        let (_d, s) = open_tmp();
        s.set_max_doc_bytes(64);
        let big = Doc::new();
        big.get_or_insert_map(REVIEWED)
            .insert(&mut big.transact_mut(), "h", "x".repeat(200));
        let upd = big
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let msg = Message::Sync(SyncMessage::Update(upd)).encode_v1();
        s.handle(&msg).expect("first update overshoots but applies");
        assert!(s.handle(&msg).is_err());
    }

    #[test]
    fn unknown_status_values_are_ignored() {
        let (_d, s) = open_tmp();
        let st = s.doc.get_or_insert_map(STATUS);
        st.insert(&mut s.doc.transact_mut(), "c9", "Bogus");
        assert!(s.snapshot().expect("snapshot").status.is_empty());
    }
}
