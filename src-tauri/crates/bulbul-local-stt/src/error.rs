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
