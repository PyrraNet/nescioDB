//! The data model: what nescioDB stores and reasons about.
//!
//! - [`domain`] — slot domains, the discretized state spaces regions live in
//! - [`evidence`] — sources, claims, and decay physics
//! - [`coupling`] — declarative cross-slot compatibility rules

pub mod coupling;
pub mod domain;
pub mod evidence;
