//! The magnustools Toolforge tool: its web service and its background jobs.
//!
//! Errors are values here, not panics. The lints below keep production code
//! that way; tests may `unwrap` freely.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

use regex::Regex;

/// Compiles one of our constant regular expressions.
///
/// Patterns never come from user input, and the tests exercise them, so an
/// invalid pattern is a programming error. This is the only deliberate
/// `expect` in production code, kept in one place to be easy to audit.
#[allow(clippy::expect_used, reason = "patterns are constants, checked by tests")]
pub(crate) fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("invalid constant regex")
}

pub mod buggregator;
pub mod cache;
pub mod config;
pub mod jobs;
pub mod mediawiki;
pub mod oauth;
pub mod public_web;
pub mod replicas;
pub mod state;
pub mod tool_db;
pub mod tools;
pub mod web;
pub mod wiki;
