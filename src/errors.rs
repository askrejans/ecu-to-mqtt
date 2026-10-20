//! Decoder errors. Transport, configuration and output errors use `anyhow`
//! with context; only protocol decoding needs a typed error.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("packet too short: expected at least {expected} bytes, got {actual}")]
    Short { expected: usize, actual: usize },
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, DecodeError>;
