#![deny(clippy::unwrap_used)]
//! Lens: intent-aware, risk-ranked, live multi-reviewer code review.

pub mod analyze;
pub mod config;
pub mod deps;
pub mod fingerprint;
pub mod git;
pub mod hash;
pub mod lang;
pub mod model;
pub mod patch;
pub mod risk;
pub mod symbols;
