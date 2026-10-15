//! Image acquisition. The settings, errors, page handling and the backend registry are the
//! `craft-scan` crate, re-exported here so the app and the tools share one surface. The scanner
//! backends are registered once by the app ([`craft_scan::register`]); with none registered every
//! request answers [`ScanError::Unsupported`].
//!
//! [`acquire`] and [`ProgressToken`] deliver in-memory [`Page`] buffers. Path helpers such as
//! [`write_pages`] are optional sinks for callers that want files; bindings that stay in memory
//! never need them.

pub use craft_scan::*;
