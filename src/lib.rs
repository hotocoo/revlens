#![deny(clippy::unwrap_used)]
//! Lens: intent-aware, risk-ranked, live multi-reviewer code review.

pub mod analyze;
pub mod config;
pub mod conformance;
pub mod deps;
pub mod fingerprint;
pub mod git;
pub mod hash;
pub mod intent;
pub mod lang;
pub mod live;
pub mod model;
pub mod patch;
pub mod reuse;
pub mod risk;
pub mod server;
pub mod session_cli;
pub mod symbols;
pub mod time;
pub mod token;
