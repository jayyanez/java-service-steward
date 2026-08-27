// SPDX-License-Identifier: Apache-2.0 OR MIT
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid arguments: {0}")]
    Cli(String),

    #[error("could not read the configuration file {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid configuration in {path}:{line}: {message}")]
    ConfigSyntax {
        path: PathBuf,
        line: usize,
        message: String,
    },

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("control channel error: {0}")]
    Protocol(String),

    #[error("Windows service error: {0}")]
    Service(String),

    #[error("diagnostic capability unavailable: {0}")]
    DiagnosticUnavailable(String),

    #[error("this operation is not supported on the current platform: {0}")]
    UnsupportedPlatform(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
