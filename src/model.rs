//! Shared data model. Serialized as JSON for the CLI, the HTTP API and the conformance record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `"h_" + 12 hex` of a stable hash of (path, old_start, new_start, normalized +/- lines).
pub type HunkId = String;
/// `"c1"`, `"c2"`, ... in intent order.
pub type CritId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Analysis {
    pub range: Range,
    pub files: Vec<FileDiff>,
    pub hunks: Vec<Hunk>,
    pub symbols: Vec<SymbolChange>,
    pub criteria: Vec<Criterion>,
    pub assumptions: Vec<Assumption>,
}

/// Lens always diffs `merge_base..head`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Range {
    pub base: String,
    pub head: String,
    pub merge_base: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub lang: Option<Lang>,
    pub generated: bool,
}

impl FileDiff {
    /// Path to show for this file: the new path, or the old one for deletions.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed { similarity: u8 },
    Copied { similarity: u8 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Lang {
    TypeScript,
    Tsx,
    Python,
    Rust,
    Go,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LineKind {
    #[serde(rename = " ")]
    Context,
    #[serde(rename = "+")]
    Added,
    #[serde(rename = "-")]
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hunk {
    pub id: HunkId,
    pub path: String,
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    pub lines: Vec<Line>,
    pub kind: HunkKind,
    /// Indexes into `Analysis::symbols`.
    pub symbols: Vec<usize>,
    pub signals: Vec<RiskSignal>,
    pub score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum HunkKind {
    Changed,
    New,
    Deleted,
    Moved { from: Loc },
    FormatOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Loc {
    pub path: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolChange {
    pub name: String,
    pub kind: SymKind,
    pub path: String,
    pub public: bool,
    pub change: Change,
    pub old_sig: Option<String>,
    pub new_sig: Option<String>,
    pub hunks: Vec<HunkId>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SymKind {
    Function,
    Method,
    Class,
    Interface,
    TypeAlias,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum Change {
    Added,
    Removed,
    BodyChanged,
    SignatureChanged,
    Renamed { from: String },
    Moved { from: Loc },
    MovedAndChanged { from: Loc },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskSignal {
    pub rule: Rule,
    pub points: u32,
    /// Shown verbatim in the UI.
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum Rule {
    PublicSigChanged,
    TestRemoved,
    NewDependency,
    SensitiveArea,
    ReuseMiss { existing: Loc },
    SuppressionAdded,
    ErrorHandlingWeakened,
    NoTestTouch,
    InfraConfig,
    Size,
    Moved,
    FormatOnly,
    Generated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Criterion {
    pub id: CritId,
    pub text: String,
    /// 1-based line in the intent document.
    pub source_line: u32,
}

/// One JSON object per line in `.revlens/assumptions.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assumption {
    pub text: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mapping {
    pub criterion: CritId,
    pub hunk: HunkId,
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claim {
    pub user: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum CritStatus {
    #[default]
    Open,
    Met,
    NotMet,
    NotApplicable,
}

/// Snapshot of the live review session (the yrs document), used for export and tests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionState {
    pub status: BTreeMap<CritId, CritStatus>,
    pub mappings: Vec<Mapping>,
    /// hunk id -> user who marked it reviewed.
    pub reviewed: BTreeMap<HunkId, String>,
    pub claims: BTreeMap<HunkId, Claim>,
    /// Comment id -> comment. Replies point at their thread's root through `parent`.
    #[serde(default)]
    pub comments: BTreeMap<String, Comment>,
    /// Root comment id -> user who resolved the thread.
    #[serde(default)]
    pub resolved: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Comment {
    pub hunk: HunkId,
    /// New-side line the thread is on; `None` for the hunk as a whole.
    #[serde(default)]
    pub line: Option<u32>,
    /// Root comment id for a reply.
    #[serde(default)]
    pub parent: Option<String>,
    pub user: String,
    pub text: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConformanceRecord {
    pub schema: u32,
    pub range: Range,
    pub generated_at: String,
    pub reviewers: Vec<String>,
    pub criteria: Vec<CriterionResult>,
    /// Hunks mapped to no criterion (excluding Moved / FormatOnly / generated).
    pub scope_creep: Vec<HunkRef>,
    /// Criteria mapped to no hunk.
    pub missing_work: Vec<CritId>,
    pub unreviewed: Vec<HunkRef>,
    pub reuse_misses: Vec<HunkRef>,
    /// Comment threads nobody resolved (added in 0.2.0; absent in older records).
    #[serde(default)]
    pub open_threads: Vec<ThreadRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadRef {
    pub id: String,
    pub path: String,
    pub line: u32,
    pub user: String,
    pub text: String,
    pub replies: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CriterionResult {
    pub criterion: Criterion,
    pub status: CritStatus,
    pub hunks: Vec<HunkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HunkRef {
    pub id: HunkId,
    pub path: String,
    pub new_start: u32,
    pub score: u32,
}

impl Hunk {
    /// "moved from X" for a hunk that adds the code, "moved to X" for one that only removes it.
    pub fn move_label(&self, other: &Loc) -> String {
        let removed_first = self
            .changed()
            .first()
            .is_some_and(|(k, _, _)| *k == LineKind::Removed);
        let dir = if removed_first { "to" } else { "from" };
        format!("moved {dir} {}:{}", other.path, other.line)
    }

    /// Added and removed lines with their line number (new side for added, old side for removed).
    pub fn changed(&self) -> Vec<(LineKind, u32, &str)> {
        let (mut old, mut new) = (self.old_start, self.new_start);
        let mut out = Vec::new();
        for l in &self.lines {
            match l.kind {
                LineKind::Context => {
                    old += 1;
                    new += 1;
                }
                LineKind::Removed => {
                    out.push((LineKind::Removed, old, l.text.as_str()));
                    old += 1;
                }
                LineKind::Added => {
                    out.push((LineKind::Added, new, l.text.as_str()));
                    new += 1;
                }
            }
        }
        out
    }
}
