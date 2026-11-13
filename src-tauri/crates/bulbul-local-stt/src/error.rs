// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the model catalog is invalid: {0}")]
    InvalidCatalog(String),

    #[error("unknown model \"{0}\"")]
    UnknownModel(String),

    #[error("\"{0}\" isn't a safe model id")]
    UnsafeId(String),

    #[error("model \"{0}\" is not installed")]
    NotInstalled(String),

    #[error("no model is loaded")]
    NotLoaded,

    #[error("couldn't read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("couldn't download {what}: {message}")]
    Network { what: String, message: String },

    #[error("the download was cancelled")]
    Cancelled,

    #[error("{file} didn't match its expected checksum, so it was discarded")]
    HashMismatch { file: String },

    #[error("not enough free disk space: {needed} bytes needed, {available} available")]
    NotEnoughSpace { needed: u64, available: u64 },

    #[error("couldn't unpack the download: {0}")]
    Archive(String),

    #[error("the speech worker stopped unexpectedly: {0}")]
    WorkerGone(String),

    #[error("the speech worker took too long: {0}")]
    Timeout(String),

    #[error("the audio can't be used: {0}")]
    BadAudio(String),

    #[error("the speech engine failed: {0}")]
    Engine(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path: path.into(), source }
    }
}
