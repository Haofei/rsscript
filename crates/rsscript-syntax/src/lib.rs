#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

pub mod ast;
