// SPDX-License-Identifier: GPL-3.0-or-later
//! Rust port of an ISPF-style editor.
//!
//! This crate owns the line
//! array, exclude blocks, undo, primary and line commands, and the keyboard
//! state the window uses. File manager, key maps, and macros are still ahead.
//! The old `SPFLite.CFG` database is not read.
//!
//! Line numbers follow the original layout. Line 1 is Top of Data, the last
//! line is Bottom of Data, and a sequence number such as `2` is the second
//! data line rather than the array index.

#![forbid(unsafe_code)]

mod cobol;
mod commands;
mod document;
mod error;
mod exclude;
pub mod highlight;
mod io;
mod line;
mod more;
mod session;
mod view;

pub use cobol::{
    cobol_outline, cobol_path, copy_name_links, definition_at, name_index, paragraph_calls,
    paragraph_index, variable_index, variable_uses, CobolEntry, CobolKind, DefSite, NameIndex,
    ParagraphCall, VariableDef,
};
pub use commands::{execute, help_text};
pub use document::{DisplayRow, Document, PendingBlock, Profile};
pub use error::{EditError, EditResult};
pub use highlight::color_line;
pub use io::{Encoding, Newline};
pub use line::{Line, LineFlags};
pub use session::{
    same_file, swapbar_fields, swapbar_window, PendingClose, Session, SwapField, SwapWindow, Tab,
};
pub use view::{EditorKey, EditorView, Focus, NextLineCursor, PointerTarget, ScrollDir};
