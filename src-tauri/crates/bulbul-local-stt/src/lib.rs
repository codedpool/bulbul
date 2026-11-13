// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! On-device speech-to-text for Bulbul.
//!
//! This crate knows *which* models exist ([`catalog`]), *where* the user's
//! copies live and whether they're complete ([`store`]), and *how to talk to
//! an engine* ([`transcriber`]). It does not link a speech engine itself: the
//! engine runs in a separate worker program so a crash there can't take the
//! app down, and so the main app never carries its native code.
//!
//! No Tauri and no Bulbul types here, so the crate can move out of the
//! repository later without changes.

pub mod catalog;
pub mod error;
pub mod store;
pub mod testing;
pub mod transcriber;

pub use catalog::{ArchiveFormat, Catalog, License, ModelEntry, ModelKind, RemoteFile, Source};
pub use error::{Error, Result};
pub use store::{Status, Store};
pub use transcriber::{Audio, LoadOptions, TranscribeOptions, Transcriber, Transcript};
