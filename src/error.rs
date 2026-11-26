// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors returned by the editor engine.

use std::fmt;

/// A failed edit or command. `message` is what the status line would show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    pub message: String,
}

impl EditError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EditError {}

pub type EditResult<T> = Result<T, EditError>;
