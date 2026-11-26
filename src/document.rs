// SPDX-License-Identifier: GPL-3.0-or-later
//! The edit document.
//!
//! Line 0 is unused so a line number is the same index SPFLite used: line 1 is
//! Top of Data and the last line is Bottom of Data. Data lines sit between
//! them. Sequence numbers (`00000001` ...) are what `GetLineNumber` searched
//! for; they are not the array index.

use crate::error::{EditError, EditResult};
use crate::io::{self, Encoding, Newline};
use crate::line::{Line, LineFlags};

const MAX_UNDO: usize = 200;

/// Furthest column the cursor may occupy, past the last non-space on a line.
pub const MAX_COLUMN: usize = 4096;

/// Profile values that change how text is stored and searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Pad data lines out to this many columns. Zero disables padding.
    pub min_len: usize,
    /// First column searched, 1-based.
    pub bounds_left: usize,
    /// Last column searched, 1-based. Zero means through the end of the line.
    pub bounds_right: usize,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            min_len: 0,
            bounds_left: 1,
            bounds_right: 0,
        }
    }
}

/// A line command waiting for its partner.
///
/// `DD` and `XX` wait for the other end of the block. `C`/`CC` and `M`/`MM`
/// wait for `A` or `B`, and a destination waits for a source. An open `CC` or
/// `MM` is only the first mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingBlock {
    None,
    Delete(usize),
    Exclude(usize),
    /// `C` or `Cn` on one visible line. `lines` is the typed count.
    CopyLine {
        line: usize,
        lines: usize,
    },
    /// First `CC`, still waiting for the other end.
    CopyOpen {
        line: usize,
        repeat: usize,
    },
    /// A finished `CC` range, waiting for `A` or `B`.
    CopySpan {
        start: usize,
        end: usize,
        repeat: usize,
    },
    MoveLine {
        line: usize,
        lines: usize,
    },
    MoveOpen {
        line: usize,
        repeat: usize,
    },
    MoveSpan {
        start: usize,
        end: usize,
        repeat: usize,
    },
    After {
        line: usize,
        repeat: usize,
    },
    Before {
        line: usize,
        repeat: usize,
    },
    /// First `RR`. `times` is 0 when the mark had no number.
    Replicate {
        line: usize,
        times: usize,
    },
    /// First `UCC`.
    Upper(usize),
    /// First `LCC`.
    Lower(usize),
    /// First `((`, `))`, `<<`, or `>>`.
    Shift {
        line: usize,
        cols: usize,
        right: bool,
        data: bool,
    },
    /// First half of a paired block such as `JJ` or `UU`.
    Pair {
        line: usize,
        kind: PairKind,
        count: usize,
    },
    /// A one-line destination such as `O`, `OR`, `H`, or `W`.
    DestLine {
        line: usize,
        kind: DestKind,
        repeat: usize,
    },
    /// First `OO`, `HH`, `AA`, `BB`, or `WW`.
    DestOpen {
        line: usize,
        kind: DestKind,
        repeat: usize,
    },
    /// A finished block destination waiting for `C` or `M`.
    DestSpan {
        start: usize,
        end: usize,
        kind: DestKind,
        repeat: usize,
    },
}

/// Block line commands that pair like `DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairKind {
    Join,
    Glue,
    TextJoin,
    TextGlue,
    Sentence,
    Title,
    Trim,
    Pad,
    Trunc,
    IndentLeft,
    IndentRight,
    Flow,
    Margin,
    MakeNote,
    MakeData,
    User,
    Revert,
    ToggleX,
    ToggleUser,
}

/// Destinations other than `A` and `B`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestKind {
    Overlay,
    OverlayReplace,
    Here,
    AfterEach,
    BeforeEach,
    Swap,
}

/// How a copy or move shifted later line numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineShift {
    pub insert_after: usize,
    pub inserted: usize,
    pub delete_at: usize,
    pub deleted: usize,
}

impl LineShift {
    pub(crate) fn identity() -> Self {
        Self {
            insert_after: 0,
            inserted: 0,
            delete_at: 0,
            deleted: 0,
        }
    }

    /// Map a line index from before the edit. `None` if that line was deleted.
    pub(crate) fn map(self, mut line: usize) -> Option<usize> {
        if self.inserted > 0 && line > self.insert_after {
            line += self.inserted;
        }
        if self.deleted > 0 {
            let delete_end = self.delete_at + self.deleted - 1;
            if line >= self.delete_at && line <= delete_end {
                return None;
            }
            if line > delete_end {
                line -= self.deleted;
            }
        }
        Some(line)
    }
}

impl PendingBlock {
    pub(crate) fn is_relocate(self) -> bool {
        matches!(
            self,
            PendingBlock::CopyLine { .. }
                | PendingBlock::CopyOpen { .. }
                | PendingBlock::CopySpan { .. }
                | PendingBlock::MoveLine { .. }
                | PendingBlock::MoveOpen { .. }
                | PendingBlock::MoveSpan { .. }
                | PendingBlock::After { .. }
                | PendingBlock::Before { .. }
                | PendingBlock::DestLine { .. }
                | PendingBlock::DestOpen { .. }
                | PendingBlock::DestSpan { .. }
        )
    }

    /// Lines whose prefix should show this pending command.
    pub(crate) fn covers(self, line: usize) -> bool {
        match self {
            PendingBlock::Delete(n)
            | PendingBlock::Exclude(n)
            | PendingBlock::CopyLine { line: n, .. }
            | PendingBlock::CopyOpen { line: n, .. }
            | PendingBlock::MoveLine { line: n, .. }
            | PendingBlock::MoveOpen { line: n, .. }
            | PendingBlock::After { line: n, .. }
            | PendingBlock::Before { line: n, .. }
            | PendingBlock::Replicate { line: n, .. }
            | PendingBlock::Upper(n)
            | PendingBlock::Lower(n)
            | PendingBlock::Shift { line: n, .. }
            | PendingBlock::Pair { line: n, .. }
            | PendingBlock::DestLine { line: n, .. }
            | PendingBlock::DestOpen { line: n, .. } => n == line,
            PendingBlock::DestSpan { start, end, .. } => line == start || line == end,
            PendingBlock::CopySpan { start, end, .. }
            | PendingBlock::MoveSpan { start, end, .. } => line == start || line == end,
            PendingBlock::None => false,
        }
    }

    /// Prefix text while this command is waiting. `DD` and `XX` are included
    /// so the first mark stays visible after Enter.
    pub fn prefix_text(self, line: usize) -> Option<String> {
        let text = match self {
            PendingBlock::Delete(n) if n == line => "DD".to_string(),
            PendingBlock::Exclude(n) if n == line => "XX".to_string(),
            PendingBlock::CopyLine { line: n, lines } if n == line => numbered("C", lines),
            PendingBlock::MoveLine { line: n, lines } if n == line => numbered("M", lines),
            PendingBlock::CopyOpen { line: n, repeat } if n == line => numbered_block("CC", repeat),
            PendingBlock::MoveOpen { line: n, repeat } if n == line => numbered_block("MM", repeat),
            PendingBlock::CopySpan { start, end, repeat } if line == start || line == end => {
                numbered_block("CC", repeat)
            }
            PendingBlock::MoveSpan { start, end, repeat } if line == start || line == end => {
                numbered_block("MM", repeat)
            }
            PendingBlock::After { line: n, repeat } if n == line => numbered_block("A", repeat),
            PendingBlock::Before { line: n, repeat } if n == line => numbered_block("B", repeat),
            PendingBlock::Replicate { line: n, times } if n == line => numbered_block("RR", times),
            PendingBlock::Upper(n) if n == line => "UCC".to_string(),
            PendingBlock::Lower(n) if n == line => "LCC".to_string(),
            PendingBlock::Shift {
                line: n,
                cols,
                right,
                data,
            } if n == line => shift_mark(right, data, cols),
            PendingBlock::Pair { line: n, kind, .. } if n == line => pair_name(kind).to_string(),
            PendingBlock::DestLine {
                line: n,
                kind,
                repeat,
            } if n == line => numbered_block(dest_name(kind, false), repeat),
            PendingBlock::DestOpen {
                line: n,
                kind,
                repeat,
            } if n == line => numbered_block(dest_name(kind, true), repeat),
            PendingBlock::DestSpan {
                start,
                end,
                kind,
                repeat,
            } if line == start || line == end => numbered_block(dest_name(kind, true), repeat),
            _ => return None,
        };
        Some(text)
    }
}

fn numbered(letter: &str, lines: usize) -> String {
    if lines <= 1 {
        letter.to_string()
    } else {
        format!("{letter}{lines}")
    }
}

fn shift_mark(right: bool, data: bool, cols: usize) -> String {
    let letters = match (data, right) {
        (false, false) => "((",
        (false, true) => "))",
        (true, false) => "<<",
        (true, true) => ">>",
    };
    numbered_block(letters, cols)
}

fn numbered_block(letters: &str, repeat: usize) -> String {
    if repeat == 0 {
        letters.to_string()
    } else {
        format!("{letters}{repeat}")
    }
}

pub(crate) fn pair_name(kind: PairKind) -> &'static str {
    match kind {
        PairKind::Join => "JJ",
        PairKind::Glue => "GG",
        PairKind::TextJoin => "TJJ",
        PairKind::TextGlue => "TGG",
        PairKind::Sentence => "SCC",
        PairKind::Title => "TCC",
        PairKind::Trim => "TRR",
        PairKind::Pad => "PLL",
        PairKind::Trunc => "TLL",
        PairKind::IndentLeft => "[[",
        PairKind::IndentRight => "]]",
        PairKind::Flow => "TFF",
        PairKind::Margin => "TMM",
        PairKind::MakeNote => "MNN",
        PairKind::MakeData => "MDD",
        PairKind::User => "UU",
        PairKind::Revert => "VV",
        PairKind::ToggleX => "TXX",
        PairKind::ToggleUser => "TUU",
    }
}

pub(crate) fn dest_name(kind: DestKind, block: bool) -> &'static str {
    match (kind, block) {
        (DestKind::Overlay, false) => "O",
        (DestKind::Overlay, true) => "OO",
        (DestKind::OverlayReplace, false) => "OR",
        (DestKind::OverlayReplace, true) => "ORR",
        (DestKind::Here, false) => "H",
        (DestKind::Here, true) => "HH",
        (DestKind::AfterEach, _) => "AA",
        (DestKind::BeforeEach, _) => "BB",
        (DestKind::Swap, false) => "W",
        (DestKind::Swap, true) => "WW",
    }
}

impl Default for PendingBlock {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Clone)]
struct Snapshot {
    lines: Vec<Line>,
    cursor_line: usize,
    cursor_col: usize,
    cursor_on_find: bool,
    modified: bool,
    pending: PendingBlock,
    profile: Profile,
    encoding: Encoding,
    newline: Newline,
    trailing_newline: bool,
    find_serial: i32,
    find_marks: Vec<FindSpan>,
    number_on: bool,
    number_std: bool,
    number_cobol: bool,
    number_display: bool,
}

/// One file being edited.
#[derive(Clone)]
pub struct Document {
    pub(crate) lines: Vec<Line>,
    cursor_line: usize,
    cursor_col: usize,
    /// Set when FIND left the cursor on the match, so the next FIND skips it.
    cursor_on_find: bool,
    modified: bool,
    pending: PendingBlock,
    profile: Profile,
    encoding: Encoding,
    newline: Newline,
    trailing_newline: bool,
    find_serial: i32,
    /// Columns highlighted by the latest FIND. `col` is 1-based.
    find_marks: Vec<FindSpan>,
    /// Last FIND string, case flag, and qualifiers other than direction.
    recall_find: Option<(String, bool, String)>,
    /// RFIND ran off this edge. The next RFIND in the same direction, still
    /// on this cursor, starts from the other end. `(upward, line, col)`.
    rfind_edge: Option<(bool, usize, usize)>,
    /// Last CHANGE from-string, case flag, replacement, and qualifiers.
    recall_change: Option<(String, bool, String, String)>,
    /// Last LOCATE operand.
    recall_locate: Option<String>,
    /// First line on the screen. FIND and CHANGE NEXT start there.
    screen_find_line: Option<usize>,
    hex: bool,
    /// Fixed column ruler. Not part of the file and not undone.
    cols: bool,
    /// ISPF number mode. Sequence numbers then run 100, 200, 300, ...
    number_on: bool,
    number_std: bool,
    number_cobol: bool,
    number_display: bool,
    next_track: i32,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// Empty edit: Top of Data, Bottom of Data, cursor on the top line.
    pub fn new() -> Self {
        let mut doc = Self {
            lines: Vec::new(),
            cursor_line: 1,
            cursor_col: 1,
            cursor_on_find: false,
            modified: false,
            pending: PendingBlock::None,
            profile: Profile::default(),
            encoding: Encoding::Utf8,
            newline: Newline::platform(),
            trailing_newline: true,
            find_serial: 0,
            find_marks: Vec::new(),
            recall_find: None,
            rfind_edge: None,
            recall_change: None,
            recall_locate: None,
            screen_find_line: None,
            hex: false,
            cols: false,
            number_on: false,
            number_std: false,
            number_cobol: false,
            number_display: false,
            next_track: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        };
        let blank = doc.alloc_track();
        let top = doc.alloc_track();
        let bottom = doc.alloc_track();
        doc.lines.push(Line::blank(0, blank));
        doc.lines.push(Line::top(top));
        doc.lines.push(Line::bottom(bottom));
        doc
    }

    pub fn from_text(text: &str) -> Self {
        Self::from_bytes(text.as_bytes())
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let decoded = io::decode_bytes(bytes);
        let newline = if decoded.text.contains("\r\n") {
            Newline::Crlf
        } else {
            Newline::Lf
        };
        let (body, trailing) = io::split_lines(&decoded.text);
        let mut doc = Self::new();
        doc.encoding = decoded.encoding;
        doc.newline = newline;
        doc.trailing_newline = trailing;
        for (i, text) in body.iter().enumerate() {
            let mut line = Line::data(doc.alloc_track());
            line.set_text(text);
            doc.lines.insert(2 + i, line);
        }
        doc.renumber();
        doc.place_initial_cursor();
        doc
    }

    pub fn to_text(&self) -> String {
        let body: Vec<String> = self
            .lines
            .iter()
            .filter(|line| line.is_data())
            .map(|line| line.text().to_string())
            .collect();
        io::join_lines(
            &body,
            self.newline,
            self.trailing_newline && !body.is_empty(),
        )
    }

    pub fn to_bytes(&self) -> EditResult<Vec<u8>> {
        io::encode_text(&self.to_text(), self.encoding).map_err(|ch| {
            EditError::new(format!(
                "Character '{ch}' cannot be stored as {}",
                self.encoding.name()
            ))
        })
    }

    pub fn load_path(path: &std::path::Path) -> EditResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|err| EditError::new(format!("Cannot open {}: {err}", path.display())))?;
        Ok(Self::from_bytes(&bytes))
    }

    pub fn save_path(&mut self, path: &std::path::Path) -> EditResult<()> {
        let bytes = self.to_bytes()?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|err| {
                    EditError::new(format!("Cannot create {}: {err}", parent.display()))
                })?;
            }
        }
        std::fs::write(path, bytes)
            .map_err(|err| EditError::new(format!("Cannot save {}: {err}", path.display())))?;
        self.modified = false;
        Ok(())
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    pub fn set_encoding(&mut self, encoding: Encoding) {
        self.encoding = encoding;
    }

    pub fn newline(&self) -> Newline {
        self.newline
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn set_bounds(&mut self, left: usize, right: usize) -> EditResult<()> {
        if left == 0 {
            return Err(EditError::new("Bounds column must be 1 or greater"));
        }
        if right != 0 && right < left {
            return Err(EditError::new("Right bound is before the left bound"));
        }
        self.push_undo(false);
        self.profile.bounds_left = left;
        self.profile.bounds_right = right;
        Ok(())
    }

    pub fn set_min_len(&mut self, min_len: usize) {
        self.push_undo(true);
        self.profile.min_len = min_len;
        let last = self.last_line();
        for i in 1..=last {
            if self.lines[i].is_data() {
                self.lines[i].pad_to(min_len);
            }
        }
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    pub fn pending(&self) -> PendingBlock {
        self.pending
    }

    pub(crate) fn set_pending(&mut self, pending: PendingBlock) {
        self.pending = pending;
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_line, self.cursor_col)
    }

    /// The editor tells FIND and CHANGE which line is at the top of the screen.
    pub(crate) fn set_screen_find_line(&mut self, line: Option<usize>) {
        self.screen_find_line = line;
    }

    pub(crate) fn screen_find_line(&self) -> Option<usize> {
        self.screen_find_line
    }

    pub fn cursor_on_find(&self) -> bool {
        self.cursor_on_find
    }

    pub fn set_cursor(&mut self, line: usize, col: usize) -> EditResult<()> {
        self.ensure_line(line)?;
        self.cursor_line = line;
        self.cursor_col = col.max(1);
        self.cursor_on_find = false;
        self.rfind_edge = None;
        self.clamp_cursor();
        Ok(())
    }

    pub(crate) fn place_found_cursor(&mut self, line: usize, col: usize) {
        self.cursor_line = line;
        self.cursor_col = col.max(1);
        self.cursor_on_find = true;
        self.clamp_cursor();
    }

    pub fn last_line(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    /// How many real data lines exist, hidden ones included.
    pub fn last_real(&self) -> usize {
        self.lines.iter().filter(|line| line.is_data()).count()
    }

    pub fn line(&self, n: usize) -> EditResult<&Line> {
        self.lines
            .get(n)
            .filter(|_| n >= 1)
            .ok_or_else(|| EditError::new(format!("Line {n} is not in the file")))
    }

    pub fn line_text(&self, n: usize) -> EditResult<&str> {
        Ok(self.line(n)?.text())
    }

    pub fn max_length(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.is_data())
            .map(Line::char_len)
            .max()
            .unwrap_or(0)
    }

    /// `0` and `.0` are Top of Data. `.ZF` is line 2. `.ZL` is the line
    /// before Bottom. A number is a sequence number, not an array index.
    /// `!n` is the raw index, which is how SPFLite's `!` line pointer worked.
    pub fn resolve_line(&self, operand: &str) -> EditResult<usize> {
        let oper = operand.trim().to_ascii_uppercase();
        if oper.is_empty() {
            return Err(EditError::new("Missing line reference"));
        }
        if oper.chars().count() > 8 && !oper.starts_with('!') {
            return Err(EditError::new("Line reference is longer than 8 characters"));
        }
        if oper == "0" || oper == ".0" {
            return Ok(1);
        }
        if let Some(rest) = oper.strip_prefix('!') {
            let n = parse_positive(rest)?;
            self.ensure_line(n)?;
            return Ok(n);
        }
        if let Some(rest) = oper.strip_prefix('.') {
            if rest == "ZFIRST" || rest == "ZF" {
                if self.last_line() <= 2 {
                    return Err(EditError::new("File is empty"));
                }
                return Ok(2);
            }
            if rest == "ZLAST" || rest == "ZL" {
                if self.last_line() <= 2 {
                    return Err(EditError::new("File is empty"));
                }
                return Ok(self.last_line() - 1);
            }
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                let n = parse_positive(rest)?;
                return self.find_seq(n);
            }
            return self.find_label(&oper);
        }
        if oper.starts_with(':') {
            return self.find_tag(&oper);
        }
        if oper.chars().all(|c| c.is_ascii_digit()) {
            return self.find_seq(parse_positive(&oper)?);
        }
        Err(EditError::new(format!(
            "Invalid line reference '{operand}'"
        )))
    }

    pub fn set_label(&mut self, line: usize, label: Option<&str>) -> EditResult<()> {
        self.ensure_data_target(line)?;
        let normalized = match label {
            None => None,
            Some(raw) => Some(normalize_name(raw, '.')?),
        };
        self.push_undo(false);
        if let Some(name) = &normalized {
            for other in &mut self.lines {
                if other.label.as_deref() == Some(name.as_str()) {
                    other.label = None;
                }
            }
        }
        self.lines[line].label = normalized;
        Ok(())
    }

    pub fn set_tag(&mut self, line: usize, tag: Option<&str>) -> EditResult<()> {
        self.ensure_data_target(line)?;
        let normalized = match tag {
            None => None,
            Some(raw) => Some(normalize_name(raw, ':')?),
        };
        self.push_undo(false);
        self.lines[line].tag = normalized;
        Ok(())
    }

    pub fn set_line_text(&mut self, line: usize, text: &str) -> EditResult<()> {
        let row = self.line(line)?;
        if !row.is_data() && !row.is_note() {
            return Err(EditError::new("Cannot type on this line"));
        }
        self.push_undo(true);
        self.lines[line].set_text(text);
        self.lines[line].flags.remove(LineFlags::INSERT);
        self.lines[line].pad_to(self.profile.min_len);
        Ok(())
    }

    pub fn insert_char(&mut self, line: usize, col: usize, ch: char) -> EditResult<()> {
        self.edit_chars(line, |chars| {
            if col == 0 || col > MAX_COLUMN {
                return Err(EditError::new("Column is outside the line"));
            }
            let at = col - 1;
            if at > chars.len() {
                chars.resize(at, ' ');
            }
            chars.insert(at, ch);
            Ok(())
        })
    }

    /// Remove this column and everything after it. A column past the line changes nothing.
    pub fn delete_from_column(&mut self, line: usize, col: usize) -> EditResult<bool> {
        let at = col.saturating_sub(1);
        {
            let row = self.line(line)?;
            if !row.is_data() && !row.is_note() {
                return Ok(false);
            }
            if at >= row.char_len() {
                return Ok(false);
            }
        }
        self.edit_chars(line, |chars| {
            chars.truncate(at);
            Ok(())
        })?;
        Ok(true)
    }

    pub fn delete_char(&mut self, line: usize, col: usize) -> EditResult<()> {
        self.edit_chars(line, |chars| {
            if col == 0 || col > chars.len() {
                return Err(EditError::new("Column is outside the line"));
            }
            chars.remove(col - 1);
            Ok(())
        })
    }

    pub fn replace_char(&mut self, line: usize, col: usize, ch: char) -> EditResult<()> {
        self.edit_chars(line, |chars| {
            if col == 0 || col > MAX_COLUMN {
                return Err(EditError::new("Column is outside the line"));
            }
            if col > chars.len() {
                chars.resize(col, ' ');
            }
            chars[col - 1] = ch;
            Ok(())
        })
    }

    /// Insert `count` lines after `after`. On an exclude marker the new lines
    /// land after the hidden block, which is the line the marker stands for.
    /// Returns the index of the first new line.
    pub fn insert_lines_after(
        &mut self,
        after: usize,
        count: usize,
        as_insert: bool,
    ) -> EditResult<usize> {
        if count == 0 {
            return Err(EditError::new("Insert count is zero"));
        }
        self.ensure_line(after)?;
        if after >= self.last_line() {
            return Err(EditError::new("Cannot insert after the bottom of data"));
        }
        let mut at = after;
        if self.lines[at].is_xclude() {
            at += self.lines[at].work1.max(0) as usize;
        }
        if at >= self.last_line() {
            return Err(EditError::new("Cannot insert after the bottom of data"));
        }
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let mask = self
            .lines
            .iter()
            .find(|line| line.flags.contains(LineFlags::MASK))
            .map(|line| line.text().to_string());
        let first = at + 1;
        for n in 0..count {
            let mut line = Line::data(self.alloc_track());
            if let Some(mask) = &mask {
                line.set_text(mask);
            }
            if as_insert {
                line.flags.insert(LineFlags::INSERT);
            }
            line.pad_to(self.profile.min_len);
            self.lines.insert(first + n, line);
        }
        self.rebuild_excludes();
        self.renumber();
        self.clamp_cursor();
        Ok(first)
    }

    /// Delete an inclusive range of raw line indexes. Top and Bottom stay.
    pub fn delete_range(&mut self, start: usize, end: usize) -> EditResult<()> {
        self.ensure_line(start)?;
        self.ensure_line(end)?;
        if end < start {
            return Err(EditError::new("Delete range is reversed"));
        }
        if self.lines[start].is_top() || self.lines[end].is_bottom() || end >= self.last_line() {
            return Err(EditError::new("Cannot delete Top or Bottom of Data"));
        }
        for line in &self.lines[start..=end] {
            if line.is_top() || line.is_bottom() {
                return Err(EditError::new("Cannot delete Top or Bottom of Data"));
            }
        }
        self.push_undo(true);
        self.pending = PendingBlock::None;
        self.lines.drain(start..=end);
        self.rebuild_excludes();
        self.renumber();
        self.clamp_cursor();
        Ok(())
    }

    /// Replace the text between two caret positions. Columns are 1-based and
    /// the end column is exclusive. `insert` may contain newlines.
    pub(crate) fn splice_text(
        &mut self,
        a_line: usize,
        a_col: usize,
        b_line: usize,
        b_col: usize,
        insert: &str,
    ) -> EditResult<()> {
        let ((start_line, start_col), (end_line, end_col)) =
            order_carets(a_line, a_col, b_line, b_col);
        self.ensure_line(start_line)?;
        self.ensure_line(end_line)?;
        for line in start_line..=end_line {
            let row = &self.lines[line];
            if !row.is_data() && !row.is_note() {
                return Err(EditError::new("Cannot change this line"));
            }
        }
        let head = {
            let chars = self.lines[start_line].chars();
            let at = start_col.saturating_sub(1);
            let mut head = chars[..at.min(chars.len())].to_vec();
            if !insert.is_empty() && at > head.len() {
                head.resize(at, ' ');
            }
            head
        };
        let tail = {
            let chars = self.lines[end_line].chars();
            let at = end_col.saturating_sub(1).min(chars.len());
            chars[at..].to_vec()
        };
        let parts: Vec<Vec<char>> = if insert.is_empty() {
            vec![Vec::new()]
        } else {
            insert
                .split('\n')
                .map(|part| part.trim_end_matches('\r').chars().collect())
                .collect()
        };
        self.push_undo(true);
        self.pending = PendingBlock::None;
        self.clear_find_highlights();
        let mut first = head.clone();
        first.extend(parts[0].iter().copied());
        let cursor_col = head.len() + 1;
        if parts.len() == 1 {
            first.extend(tail.iter().copied());
        }
        self.lines[start_line].set_chars(&first);
        self.lines[start_line].flags.remove(LineFlags::INSERT);
        self.lines[start_line].pad_to(self.profile.min_len);
        if end_line > start_line {
            self.lines.drain(start_line + 1..=end_line);
        }
        if parts.len() > 1 {
            for (offset, part) in parts.iter().enumerate().skip(1) {
                let mut chars = part.clone();
                if offset + 1 == parts.len() {
                    chars.extend(tail.iter().copied());
                }
                let mut line = Line::data(self.alloc_track());
                line.set_chars(&chars);
                line.pad_to(self.profile.min_len);
                self.lines.insert(start_line + offset, line);
            }
        }
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = start_line;
        self.cursor_col = cursor_col.max(1);
        self.cursor_on_find = false;
        self.clamp_cursor();
        Ok(())
    }

    /// `Dn` walks `repeat` visible units. An exclude marker counts as one unit
    /// and the delete also takes the hidden lines it covers.
    pub fn delete_units(&mut self, start: usize, repeat: usize) -> EditResult<()> {
        let end = self.span_end(start, repeat)?;
        self.delete_range(start, end)
    }

    /// Copy `start..=end` so the new lines sit after `dest`, or before it when
    /// `before` is set. `repeat` is how many times the range is laid down.
    /// A move deletes the original range after the copy. One undo covers it.
    pub(crate) fn copy_move(
        &mut self,
        start: usize,
        end: usize,
        dest: usize,
        before: bool,
        repeat: usize,
        delete_source: bool,
    ) -> EditResult<LineShift> {
        self.ensure_line(start)?;
        self.ensure_line(end)?;
        self.ensure_line(dest)?;
        if end < start {
            return Err(EditError::new("Copy range is reversed"));
        }
        if self.lines[start].is_top() || self.lines[start].is_bottom() {
            return Err(EditError::new("Illegal line command for this line"));
        }
        if end >= self.last_line() || self.lines[end].is_bottom() || self.lines[end].is_top() {
            return Err(EditError::new("Copy range runs past the data"));
        }
        if ranges_overlap(start, end, dest, dest) {
            return Err(EditError::new("Overlapping line ranges entered"));
        }
        let repeat = repeat.max(1);
        let dest_after = if before {
            if dest <= 1 {
                return Err(EditError::new("Cannot copy before the top of data"));
            }
            dest - 1
        } else {
            if self.lines[dest].is_bottom() {
                return Err(EditError::new("Cannot insert after the bottom of data"));
            }
            dest
        };
        let mut at = dest_after;
        if self.lines[at].is_xclude() {
            at += self.lines[at].work1.max(0) as usize;
        }
        if at == 0 || at >= self.last_line() {
            return Err(EditError::new("Cannot insert after the bottom of data"));
        }

        let count = end - start + 1;
        let copies = count * repeat;
        let prototypes = self.lines[start..=end].to_vec();
        self.push_undo(true);
        self.pending = PendingBlock::None;
        self.cursor_on_find = false;
        for n in 0..copies {
            let mut line = prototypes[n % count].clone();
            line.track = self.alloc_track();
            line.flags.remove(LineFlags::INSERT);
            line.flags.remove(LineFlags::EQ_CHANGE);
            line.flags.remove(LineFlags::CURSOR);
            line.flags.remove(LineFlags::SCROLL);
            if !(delete_source && n < count) {
                line.label = None;
            }
            self.lines.insert(at + 1 + n, line);
        }

        let (delete_at, deleted) = if delete_source {
            let delete_at = if start > at { start + copies } else { start };
            self.lines.drain(delete_at..=delete_at + count - 1);
            (delete_at, count)
        } else {
            (0, 0)
        };

        let mut first = at + 1;
        if deleted > 0 && delete_at + deleted - 1 < first {
            first -= deleted;
        }
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = first;
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(LineShift {
            insert_after: at,
            inserted: copies,
            delete_at,
            deleted,
        })
    }

    /// Rewrite data and note lines in `start..=end`. One undo covers the span.
    pub(crate) fn map_text(
        &mut self,
        start: usize,
        end: usize,
        mut edit: impl FnMut(&str) -> String,
    ) -> EditResult<()> {
        self.ensure_span_public(start, end)?;
        self.push_undo(true);
        self.pending = PendingBlock::None;
        for line in start..=end {
            if self.lines[line].is_data() || self.lines[line].is_note() {
                let next = edit(self.lines[line].text());
                self.lines[line].set_text(&next);
                self.lines[line].flags.remove(LineFlags::INSERT);
                self.lines[line].pad_to(self.profile.min_len);
            }
        }
        self.renumber();
        self.clamp_cursor();
        Ok(())
    }

    /// Sort data lines. Labels travel with the text they were on.
    pub fn sort_data(&mut self, descending: bool) -> EditResult<()> {
        let indexes: Vec<usize> = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.is_data())
            .map(|(index, _)| index)
            .collect();
        if indexes.len() < 2 {
            return Ok(());
        }
        self.push_undo(true);
        let mut rows: Vec<Line> = indexes
            .iter()
            .map(|index| self.lines[*index].clone())
            .collect();
        rows.sort_by(|left, right| {
            let order = left.text().cmp(right.text());
            if descending {
                order.reverse()
            } else {
                order
            }
        });
        for (index, row) in indexes.into_iter().zip(rows) {
            self.lines[index] = row;
        }
        self.rebuild_excludes();
        self.renumber();
        self.clamp_cursor();
        Ok(())
    }

    /// Delete these raw indexes. Top and Bottom are left in place.
    pub fn delete_lines(&mut self, lines: &[usize]) -> EditResult<usize> {
        let last = self.last_line();
        let mut doomed: Vec<usize> = lines
            .iter()
            .copied()
            .filter(|line| {
                self.lines
                    .get(*line)
                    .is_some_and(|row| !row.is_top() && !row.is_bottom() && *line < last)
            })
            .collect();
        doomed.sort_unstable();
        doomed.dedup();
        if doomed.is_empty() {
            return Err(EditError::new("No lines to delete"));
        }
        self.push_undo(true);
        self.pending = PendingBlock::None;
        for index in doomed.iter().rev() {
            self.lines.remove(*index);
        }
        let removed = doomed.len();
        self.rebuild_excludes();
        self.renumber();
        self.clamp_cursor();
        Ok(removed)
    }

    fn ensure_span_public(&self, start: usize, end: usize) -> EditResult<()> {
        self.ensure_line(start)?;
        self.ensure_line(end)?;
        if end < start || end >= self.last_line() {
            return Err(EditError::new("Line range is outside the data"));
        }
        Ok(())
    }

    pub(crate) fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Drop edits made after the undo stack was `depth` long, and return to that text.
    pub(crate) fn revert_to_depth(&mut self, depth: usize) -> bool {
        if self.undo.len() <= depth {
            return false;
        }
        let mut restored = None;
        while self.undo.len() > depth {
            restored = self.undo.pop();
        }
        if let Some(snap) = restored {
            self.restore(snap);
        }
        self.redo.clear();
        true
    }

    pub fn undo(&mut self) -> EditResult<()> {
        let previous = self
            .undo
            .pop()
            .ok_or_else(|| EditError::new("Nothing to undo"))?;
        self.redo.push(self.snapshot());
        self.restore(previous);
        Ok(())
    }

    pub fn redo(&mut self) -> EditResult<()> {
        let next = self
            .redo
            .pop()
            .ok_or_else(|| EditError::new("Nothing to redo"))?;
        self.undo.push(self.snapshot());
        self.restore(next);
        Ok(())
    }

    /// Drop blank lines created by `I` and never typed on.
    /// Returns whether the cursor was sitting on one of those lines.
    pub fn cleanup_unused_inserts(&mut self) -> bool {
        let cursor = self.cursor_line;
        let doomed: Vec<usize> = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.is_insert() && line.text().trim().is_empty())
            .map(|(index, _)| index)
            .collect();
        if doomed.is_empty() {
            return false;
        }
        let on_doomed = doomed.contains(&cursor);
        let removed_before = doomed.iter().filter(|index| **index < cursor).count();
        self.push_undo(true);
        for index in doomed.iter().rev() {
            self.lines.remove(*index);
        }
        self.cursor_line = cursor - removed_before;
        self.rebuild_excludes();
        self.renumber();
        self.clamp_cursor();
        on_doomed
    }

    pub(crate) fn clear_insert_flag(&mut self, line: usize) {
        if line < self.lines.len() {
            self.lines[line].flags.remove(LineFlags::INSERT);
        }
    }

    pub(crate) fn replace_columns(
        &mut self,
        line: usize,
        start_col: usize,
        match_chars: usize,
        replacement: &str,
    ) -> EditResult<()> {
        let chars = self.line(line)?.chars();
        let start = start_col
            .checked_sub(1)
            .ok_or_else(|| EditError::new("Column is outside the line"))?;
        let end = start + match_chars;
        if end > chars.len() {
            return Err(EditError::new("Replace extends past the line"));
        }
        let mut next = Vec::with_capacity(chars.len());
        next.extend_from_slice(&chars[..start]);
        next.extend(replacement.chars());
        next.extend_from_slice(&chars[end..]);
        self.lines[line].set_chars(&next);
        self.lines[line].flags.remove(LineFlags::INSERT);
        self.lines[line].flags.insert(LineFlags::EQ_CHANGE);
        self.lines[line].pad_to(self.profile.min_len);
        Ok(())
    }

    pub fn find_spans(&self, line: usize) -> Vec<(usize, usize)> {
        self.find_marks
            .iter()
            .filter(|mark| mark.line == line)
            .map(|mark| (mark.col, mark.len))
            .collect()
    }

    pub(crate) fn set_find_highlights(&mut self, marks: Vec<FindSpan>) {
        self.find_marks = marks;
    }

    pub(crate) fn clear_find_highlights(&mut self) {
        self.find_marks.clear();
    }

    pub(crate) fn mark_find(&mut self, line: usize) {
        self.find_serial = self.find_serial.saturating_add(1);
        if let Some(row) = self.lines.get_mut(line) {
            row.find_ix = self.find_serial;
        }
    }

    pub(crate) fn push_undo(&mut self, content_change: bool) {
        self.redo.clear();
        self.undo.push(self.snapshot());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        if content_change {
            self.modified = true;
        }
    }

    pub(crate) fn span_end(&self, start: usize, repeat: usize) -> EditResult<usize> {
        if repeat == 0 {
            return Err(EditError::new("Repeat count is zero"));
        }
        self.ensure_line(start)?;
        let mut j = start;
        let mut left = repeat;
        while left > 0 {
            if j >= self.last_line() {
                return Err(EditError::new("Past the bottom of data"));
            }
            if self.lines[j].is_xclude() {
                j += self.lines[j].work1.max(0) as usize;
            }
            if j >= self.last_line() {
                return Err(EditError::new("Past the bottom of data"));
            }
            left -= 1;
            j += 1;
        }
        Ok(j - 1)
    }

    pub(crate) fn alloc_track(&mut self) -> i32 {
        let track = self.next_track;
        self.next_track = self.next_track.saturating_add(1);
        track
    }

    pub(crate) fn renumber(&mut self) {
        if self.number_on {
            self.assign_ispf_numbers();
            if self.number_cobol {
                self.stamp_cobol_numbers();
            }
            return;
        }
        let mut seq = 0u32;
        for line in self.lines.iter_mut().skip(1) {
            if line.flags.is_numbered() {
                seq += 1;
                line.seq = seq;
            } else {
                line.seq = 0;
            }
        }
    }

    /// ISPF starts at 100 and steps by 100. Past 10 000 lines the step is
    /// smaller, and a 6-digit COBOL number shrinks as soon as 100 no longer fits.
    pub(crate) fn ispf_increment(count: usize, cobol: bool) -> u32 {
        if count == 0 {
            return 100;
        }
        let limit = if cobol { 999_999u32 } else { 99_999_999 };
        let fit = (limit / count as u32).max(1);
        if count > 10_000 {
            fit.min(99)
        } else {
            fit.min(100)
        }
    }

    pub(crate) fn set_number_mode(&mut self, on: bool, std: bool, cobol: bool, display: bool) {
        self.number_on = on;
        self.number_std = std;
        self.number_cobol = cobol;
        self.number_display = display;
    }

    pub(crate) fn number_kind(&self) -> Option<&'static str> {
        if !self.number_on {
            return None;
        }
        Some(match (self.number_std, self.number_cobol) {
            (true, true) => "STD COBOL",
            (false, true) => "COBOL",
            _ => "STD",
        })
    }

    pub(crate) fn number_display(&self) -> bool {
        self.number_display
    }

    fn assign_ispf_numbers(&mut self) {
        let count = self
            .lines
            .iter()
            .filter(|line| line.flags.is_numbered())
            .count();
        let incr = Self::ispf_increment(count, self.number_cobol);
        let mut n = incr;
        for line in self.lines.iter_mut().skip(1) {
            if line.flags.is_numbered() {
                line.seq = n;
                n = n.saturating_add(incr);
            } else {
                line.seq = 0;
            }
        }
    }

    /// COBOL numbers occupy columns 1–6 and replace whatever is already there.
    fn stamp_cobol_numbers(&mut self) {
        let mut changed = false;
        for line in &mut self.lines {
            if !line.is_data() {
                continue;
            }
            let num = format!("{:06}", line.seq.min(999_999));
            let mut chars = line.chars();
            if chars.len() < 6 {
                chars.resize(6, ' ');
            }
            let mut row_changed = false;
            for (index, ch) in num.chars().enumerate() {
                if chars[index] != ch {
                    chars[index] = ch;
                    row_changed = true;
                }
            }
            if row_changed {
                line.set_chars(&chars);
                changed = true;
            }
        }
        if changed {
            self.modified = true;
        }
    }

    fn edit_chars(
        &mut self,
        line: usize,
        edit: impl FnOnce(&mut Vec<char>) -> EditResult<()>,
    ) -> EditResult<()> {
        let row = self.line(line)?;
        if !row.is_data() && !row.is_note() {
            return Err(EditError::new("Cannot type on this line"));
        }
        let mut chars = self.lines[line].chars();
        edit(&mut chars)?;
        self.push_undo(true);
        self.lines[line].set_chars(&chars);
        self.lines[line].pad_to(self.profile.min_len);
        Ok(())
    }

    fn find_seq(&self, seq: usize) -> EditResult<usize> {
        let seq = u32::try_from(seq).unwrap_or(u32::MAX);
        self.lines
            .iter()
            .position(|line| line.seq == seq && seq != 0)
            .ok_or_else(|| EditError::new(format!("Line {seq} not found")))
    }

    fn find_label(&self, label: &str) -> EditResult<usize> {
        self.lines
            .iter()
            .position(|line| {
                line.label
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(label))
            })
            .ok_or_else(|| EditError::new(format!("Label {label} not found")))
    }

    fn find_tag(&self, tag: &str) -> EditResult<usize> {
        if tag == ":Z" || tag == ":ZF" || tag == ":ZNF" {
            return Ok(1);
        }
        self.lines
            .iter()
            .position(|line| line.tag.as_deref() == Some(tag))
            .ok_or_else(|| EditError::new(format!("Tag {tag} not found")))
    }

    fn ensure_line(&self, n: usize) -> EditResult<()> {
        if n >= 1 && n <= self.last_line() {
            Ok(())
        } else {
            Err(EditError::new(format!("Line {n} is not in the file")))
        }
    }

    fn ensure_data_target(&self, n: usize) -> EditResult<()> {
        let line = self.line(n)?;
        if line.is_data() || line.is_note() {
            Ok(())
        } else {
            Err(EditError::new("Labels and tags belong on data lines"))
        }
    }

    fn place_initial_cursor(&mut self) {
        if self.last_real() > 0 {
            self.cursor_line = 2;
            self.cursor_col = 1;
        } else {
            self.cursor_line = 1;
            self.cursor_col = 1;
        }
    }

    pub(crate) fn clamp_cursor(&mut self) {
        if self.cursor_line < 1 {
            self.cursor_line = 1;
        }
        if self.cursor_line > self.last_line() {
            self.cursor_line = self.last_line();
        }
        if self.cursor_col < 1 {
            self.cursor_col = 1;
        }
        if self.cursor_col > MAX_COLUMN {
            self.cursor_col = MAX_COLUMN;
        }
    }

    pub fn hex_mode(&self) -> bool {
        self.hex
    }

    pub fn toggle_hex(&mut self) -> bool {
        self.hex = !self.hex;
        self.hex
    }

    pub fn cols_mode(&self) -> bool {
        self.cols
    }

    pub fn set_cols(&mut self, on: bool) {
        self.cols = on;
    }

    pub(crate) fn remember_find(&mut self, text: &str, case_sensitive: bool, qualifiers: &str) {
        self.recall_find = Some((text.to_string(), case_sensitive, qualifiers.to_string()));
    }

    pub(crate) fn remembered_find(&self) -> Option<(String, bool, String)> {
        self.recall_find.clone()
    }

    pub(crate) fn arm_rfind_edge(&mut self, upward: bool, line: usize, col: usize) {
        self.rfind_edge = Some((upward, line, col));
    }

    /// Consume the edge notice when this RFIND is the same direction and the
    /// cursor has not moved. Any other repeat search leaves it in place.
    pub(crate) fn take_rfind_edge(&mut self, upward: bool, line: usize, col: usize) -> bool {
        let ready = self.rfind_edge == Some((upward, line, col));
        if ready {
            self.rfind_edge = None;
        }
        ready
    }

    pub(crate) fn clear_rfind_edge(&mut self) {
        self.rfind_edge = None;
    }

    pub(crate) fn remember_change(
        &mut self,
        from: &str,
        case_sensitive: bool,
        to: &str,
        qualifiers: &str,
    ) {
        self.recall_change = Some((
            from.to_string(),
            case_sensitive,
            to.to_string(),
            qualifiers.to_string(),
        ));
    }

    pub(crate) fn remembered_change(&self) -> Option<(String, bool, String, String)> {
        self.recall_change.clone()
    }

    pub(crate) fn remember_locate(&mut self, operand: &str) {
        self.recall_locate = Some(operand.to_string());
    }

    pub(crate) fn remembered_locate(&self) -> Option<String> {
        self.recall_locate.clone()
    }

    /// Swap which data lines are excluded.
    pub fn flip(&mut self) -> EditResult<usize> {
        let touched = self
            .lines
            .iter()
            .filter(|line| line.is_data() || line.is_note())
            .count();
        if touched == 0 {
            return Err(EditError::new("No lines to flip"));
        }
        self.push_undo(false);
        for line in &mut self.lines {
            if line.is_data() || line.is_note() {
                if line.is_invisible() {
                    line.flags.remove(LineFlags::INVISIBLE);
                } else {
                    line.flags.insert(LineFlags::INVISIBLE);
                }
            }
        }
        self.rebuild_excludes();
        self.renumber();
        self.reveal_cursor_if_hidden();
        Ok(touched)
    }

    /// Show hidden lines whose text contains `text`. An empty string shows every line.
    pub fn show_matching(&mut self, text: &str, case_sensitive: bool) -> EditResult<usize> {
        if text.is_empty() {
            return self.include_all();
        }
        let mut shown = 0usize;
        for line in &self.lines {
            if line.is_invisible() && contains_text(line.text(), text, case_sensitive) {
                shown += 1;
            }
        }
        if shown == 0 {
            return Err(EditError::new(format!("'{text}' not found")));
        }
        self.push_undo(false);
        for line in &mut self.lines {
            if line.is_invisible() && contains_text(line.text(), text, case_sensitive) {
                line.flags.remove(LineFlags::INVISIBLE);
            }
        }
        self.rebuild_excludes();
        self.renumber();
        Ok(shown)
    }

    /// Exclude every data line, or every line that contains `text`.
    pub fn hide_lines(&mut self, text: Option<&str>, case_sensitive: bool) -> EditResult<usize> {
        let wanted: Vec<usize> = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                (line.is_data() || line.is_note())
                    && !line.is_invisible()
                    && text.is_none_or(|needle| contains_text(line.text(), needle, case_sensitive))
            })
            .map(|(index, _)| index)
            .collect();
        self.exclude_lines(&wanted)
    }

    pub(crate) fn join_range(
        &mut self,
        start: usize,
        end: usize,
        sep: &str,
        squeeze: bool,
    ) -> EditResult<()> {
        self.ensure_span_public(start, end)?;
        if start == end {
            return Ok(());
        }
        for line in start..=end {
            if !self.lines[line].is_data() && !self.lines[line].is_note() {
                return Err(EditError::new("Join needs data lines"));
            }
        }
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let mut parts = Vec::new();
        for line in start..=end {
            parts.push(self.lines[line].text().to_string());
        }
        let mut joined = parts.join(sep);
        if squeeze {
            joined = squeeze_spaces(&joined);
        }
        self.lines[start].set_text(&joined);
        self.lines.drain(start + 1..=end);
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = start;
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(())
    }

    pub(crate) fn split_line(&mut self, line: usize, col: usize) -> EditResult<()> {
        self.ensure_line(line)?;
        if !self.lines[line].is_data() && !self.lines[line].is_note() {
            return Err(EditError::new("Split needs a data line"));
        }
        let chars = self.lines[line].chars();
        let at = col.saturating_sub(1).min(chars.len());
        let left: String = chars[..at].iter().collect();
        let right: String = chars[at..].iter().collect();
        self.push_undo(true);
        self.pending = PendingBlock::None;
        self.lines[line].set_text(&left);
        let mut next = Line::data(self.alloc_track());
        if self.lines[line].is_note() {
            next.flags.remove(LineFlags::DATA);
            next.flags.insert(LineFlags::NOTE);
        }
        next.set_text(&right);
        self.lines.insert(line + 1, next);
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = line + 1;
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(())
    }

    pub(crate) fn map_span(
        &mut self,
        start: usize,
        end: usize,
        edit: impl FnMut(&str) -> String,
    ) -> EditResult<()> {
        self.map_text(start, end, edit)
    }

    pub(crate) fn replace_span_with(
        &mut self,
        start: usize,
        end: usize,
        rows: &[String],
    ) -> EditResult<()> {
        self.ensure_span_public(start, end)?;
        if rows.is_empty() {
            return Err(EditError::new("Nothing to write"));
        }
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let note = self.lines[start].is_note();
        self.lines[start].set_text(&rows[0]);
        self.lines.drain(start + 1..=end);
        for (offset, text) in rows.iter().enumerate().skip(1) {
            let mut line = Line::data(self.alloc_track());
            if note {
                line.flags.remove(LineFlags::DATA);
                line.flags.insert(LineFlags::NOTE);
            }
            line.set_text(text);
            self.lines.insert(start + offset, line);
        }
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = start;
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(())
    }

    pub(crate) fn tag_lines(
        &mut self,
        start: usize,
        end: usize,
        note: Option<bool>,
        user: Option<bool>,
    ) -> EditResult<()> {
        self.ensure_span_public(start, end)?;
        self.push_undo(true);
        self.pending = PendingBlock::None;
        for line in start..=end {
            if !self.lines[line].is_data() && !self.lines[line].is_note() {
                continue;
            }
            if let Some(make_note) = note {
                if make_note {
                    self.lines[line].flags.remove(LineFlags::DATA);
                    self.lines[line].flags.insert(LineFlags::NOTE);
                } else {
                    self.lines[line].flags.remove(LineFlags::NOTE);
                    self.lines[line].flags.insert(LineFlags::DATA);
                }
            }
            if let Some(on) = user {
                if on {
                    self.lines[line].flags.insert(LineFlags::USER);
                } else {
                    self.lines[line].flags.remove(LineFlags::USER);
                }
            } else if note.is_none() {
                let flags = &mut self.lines[line].flags;
                if flags.contains(LineFlags::USER) {
                    flags.remove(LineFlags::USER);
                } else {
                    flags.insert(LineFlags::USER);
                }
            }
        }
        self.renumber();
        Ok(())
    }

    pub fn toggle_excluded(&mut self, start: usize, end: usize) -> EditResult<usize> {
        self.ensure_span_public(start, end)?;
        if start == end && self.lines[start].is_xclude() {
            let count = self.lines[start].work1.max(0) as usize;
            self.show_first(start, count.max(1))?;
            return Ok(count.max(1));
        }
        let count = (start..=end)
            .filter(|line| self.lines[*line].is_data() || self.lines[*line].is_note())
            .count();
        if count == 0 {
            return Err(EditError::new("No lines to toggle"));
        }
        self.push_undo(false);
        self.pending = PendingBlock::None;
        for line in start..=end {
            if self.lines[line].is_data() || self.lines[line].is_note() {
                if self.lines[line].is_invisible() {
                    self.lines[line].flags.remove(LineFlags::INVISIBLE);
                } else {
                    self.lines[line].flags.insert(LineFlags::INVISIBLE);
                }
            }
        }
        self.rebuild_excludes();
        self.renumber();
        self.reveal_cursor_if_hidden();
        Ok(count)
    }

    pub fn insert_profile(&mut self, after: usize, flag: u32, text: &str) -> EditResult<usize> {
        self.ensure_line(after)?;
        if after >= self.last_line() {
            return Err(EditError::new("Cannot insert after the bottom of data"));
        }
        let mut at = after;
        if self.lines[at].is_xclude() {
            at += self.lines[at].work1.max(0) as usize;
        }
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let mut line = Line::blank(flag, self.alloc_track());
        line.set_text(text);
        let index = at + 1;
        self.lines.insert(index, line);
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = index;
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(index)
    }

    pub fn insert_text_lines(&mut self, after: usize, texts: &[String]) -> EditResult<usize> {
        if texts.is_empty() {
            return Err(EditError::new("Nothing to insert"));
        }
        self.ensure_line(after)?;
        if after >= self.last_line() {
            return Err(EditError::new("Cannot insert after the bottom of data"));
        }
        let mut at = after;
        if self.lines[at].is_xclude() {
            at += self.lines[at].work1.max(0) as usize;
        }
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let first = at + 1;
        for (offset, text) in texts.iter().enumerate() {
            let mut line = Line::data(self.alloc_track());
            line.set_text(text);
            self.lines.insert(first + offset, line);
        }
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = first;
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(first)
    }

    /// Copy non-blank source columns onto the destination lines.
    /// `keep_blanks` also copies spaces, which is `OR`.
    pub(crate) fn overlay_lines(
        &mut self,
        src_start: usize,
        src_end: usize,
        dest_start: usize,
        dest_end: usize,
        keep_blanks: bool,
        delete_source: bool,
    ) -> EditResult<LineShift> {
        self.ensure_span_public(src_start, src_end)?;
        self.ensure_span_public(dest_start, dest_end)?;
        if ranges_overlap(src_start, src_end, dest_start, dest_end) {
            return Err(EditError::new("Overlapping line ranges entered"));
        }
        let source: Vec<Vec<char>> = (src_start..=src_end)
            .map(|line| self.lines[line].chars())
            .collect();
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let dest_len = dest_end - dest_start + 1;
        for (index, src) in source.iter().cycle().take(dest_len).enumerate() {
            let line = dest_start + index;
            if !self.lines[line].is_data() && !self.lines[line].is_note() {
                continue;
            }
            let mut dest = self.lines[line].chars();
            if dest.len() < src.len() {
                dest.resize(src.len(), ' ');
            }
            for (col, ch) in src.iter().enumerate() {
                if keep_blanks || !ch.is_whitespace() {
                    dest[col] = *ch;
                }
            }
            self.lines[line].set_text(&dest.into_iter().collect::<String>());
        }
        let (delete_at, deleted) = if delete_source {
            let count = src_end - src_start + 1;
            self.lines.drain(src_start..=src_end);
            (src_start, count)
        } else {
            (0, 0)
        };
        self.rebuild_excludes();
        self.renumber();
        let mut cursor = dest_start;
        if deleted > 0 && delete_at < cursor {
            cursor -= deleted;
        }
        self.cursor_line = cursor.min(self.last_line());
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(LineShift {
            insert_after: 0,
            inserted: 0,
            delete_at,
            deleted,
        })
    }

    pub(crate) fn swap_ranges(
        &mut self,
        start: usize,
        end: usize,
        dest_start: usize,
        dest_end: usize,
    ) -> EditResult<LineShift> {
        self.ensure_span_public(start, end)?;
        self.ensure_span_public(dest_start, dest_end)?;
        if ranges_overlap(start, end, dest_start, dest_end) {
            return Err(EditError::new("Overlapping line ranges entered"));
        }
        let (a0, a1, b0, b1) = if start < dest_start {
            (start, end, dest_start, dest_end)
        } else {
            (dest_start, dest_end, start, end)
        };
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let first = self.lines[a0..=a1].to_vec();
        let second = self.lines[b0..=b1].to_vec();
        let mut rebuilt = Vec::new();
        rebuilt.extend_from_slice(&self.lines[..a0]);
        rebuilt.extend(second);
        rebuilt.extend_from_slice(&self.lines[a1 + 1..b0]);
        rebuilt.extend(first);
        rebuilt.extend_from_slice(&self.lines[b1 + 1..]);
        self.lines = rebuilt;
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = a0;
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(LineShift::identity())
    }

    /// Insert a copy of `start..=end` after or before every line in the destination.
    pub(crate) fn copy_each(
        &mut self,
        start: usize,
        end: usize,
        dest_start: usize,
        dest_end: usize,
        before: bool,
        delete_source: bool,
    ) -> EditResult<LineShift> {
        self.ensure_span_public(start, end)?;
        self.ensure_span_public(dest_start, dest_end)?;
        if ranges_overlap(start, end, dest_start, dest_end) {
            return Err(EditError::new("Overlapping line ranges entered"));
        }
        let copies = self.lines[start..=end].to_vec();
        self.push_undo(true);
        self.pending = PendingBlock::None;
        let mut track = self.next_track;
        let mut rebuilt =
            Vec::with_capacity(self.lines.len() + copies.len() * (dest_end - dest_start + 1));
        for (index, line) in self.lines.iter().enumerate() {
            let in_dest = index >= dest_start && index <= dest_end;
            if in_dest && before {
                let made = fresh_copies(&copies, track);
                track += made.len() as i32;
                rebuilt.extend(made);
            }
            if !(delete_source && index >= start && index <= end) {
                rebuilt.push(line.clone());
            }
            if in_dest && !before {
                let made = fresh_copies(&copies, track);
                track += made.len() as i32;
                rebuilt.extend(made);
            }
        }
        self.next_track = track;
        self.lines = rebuilt;
        self.rebuild_excludes();
        self.renumber();
        self.cursor_line = dest_start.min(self.last_line());
        self.cursor_col = 1;
        self.clamp_cursor();
        Ok(LineShift::identity())
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
            cursor_on_find: self.cursor_on_find,
            modified: self.modified,
            pending: self.pending,
            profile: self.profile.clone(),
            encoding: self.encoding,
            newline: self.newline,
            trailing_newline: self.trailing_newline,
            find_serial: self.find_serial,
            find_marks: self.find_marks.clone(),
            number_on: self.number_on,
            number_std: self.number_std,
            number_cobol: self.number_cobol,
            number_display: self.number_display,
        }
    }

    fn restore(&mut self, snap: Snapshot) {
        self.lines = snap.lines;
        self.cursor_line = snap.cursor_line;
        self.cursor_col = snap.cursor_col;
        self.cursor_on_find = snap.cursor_on_find;
        self.modified = snap.modified;
        self.pending = snap.pending;
        self.profile = snap.profile;
        self.encoding = snap.encoding;
        self.newline = snap.newline;
        self.trailing_newline = snap.trailing_newline;
        self.find_serial = snap.find_serial;
        self.find_marks = snap.find_marks;
        self.number_on = snap.number_on;
        self.number_std = snap.number_std;
        self.number_cobol = snap.number_cobol;
        self.number_display = snap.number_display;
    }
}

/// One FIND match. `col` is the first character, counting from 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindSpan {
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

/// SPFLite's source/dest test. Touching an endpoint is allowed; landing
/// strictly inside the other range is not.
fn ranges_overlap(src_start: usize, src_end: usize, dst_start: usize, dst_end: usize) -> bool {
    if src_start < dst_start {
        src_end > dst_start
    } else {
        src_start < dst_end
    }
}

fn order_carets(
    a_line: usize,
    a_col: usize,
    b_line: usize,
    b_col: usize,
) -> ((usize, usize), (usize, usize)) {
    if (b_line, b_col) < (a_line, a_col) {
        ((b_line, b_col), (a_line, a_col))
    } else {
        ((a_line, a_col), (b_line, b_col))
    }
}

fn parse_positive(text: &str) -> EditResult<usize> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return Err(EditError::new(format!("Invalid number '{text}'")));
    }
    text.parse::<usize>()
        .map_err(|_| EditError::new(format!("Invalid number '{text}'")))
        .and_then(|n| {
            if n == 0 {
                Err(EditError::new("Line number is zero"))
            } else {
                Ok(n)
            }
        })
}

fn normalize_name(raw: &str, prefix: char) -> EditResult<String> {
    let text = raw.trim().to_ascii_uppercase();
    if text.is_empty() || text == prefix.to_string() {
        return Err(EditError::new("Name is empty"));
    }
    let body = text.strip_prefix(prefix).unwrap_or(&text);
    if body.is_empty() {
        return Err(EditError::new("Name is empty"));
    }
    if text.chars().count() > 8 {
        return Err(EditError::new("Name is longer than 8 characters"));
    }
    let mut chars = body.chars();
    let first = chars.next().unwrap();
    if first.is_ascii_digit() || first == '_' {
        return Err(EditError::new("Name cannot start with a digit"));
    }
    if body
        .chars()
        .any(|c| c.is_whitespace() || c == '\'' || c == '"' || c == ',')
    {
        return Err(EditError::new("Name contains an invalid character"));
    }
    Ok(format!("{prefix}{body}"))
}

/// A visible row, ready for a screen or the text front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRow {
    pub line: usize,
    pub prefix: String,
    pub text: String,
    pub exclude_count: Option<i32>,
}

impl Document {
    pub fn visible_rows(&self) -> Vec<DisplayRow> {
        self.lines
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, line)| !line.is_invisible())
            .map(|(line, row)| DisplayRow {
                line,
                prefix: row.display_prefix(),
                text: row.text().to_string(),
                exclude_count: row.is_xclude().then_some(row.work1),
            })
            .collect()
    }
}

fn contains_text(text: &str, needle: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        text.contains(needle)
    } else {
        text.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }
}

fn squeeze_spaces(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !blank && !out.is_empty() {
                out.push(' ');
            }
            blank = true;
        } else {
            blank = false;
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

fn fresh_copies(lines: &[Line], start_track: i32) -> Vec<Line> {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let mut copy = line.clone();
            copy.track = start_track + index as i32;
            copy.label = None;
            copy.flags.remove(LineFlags::INSERT);
            copy.flags.remove(LineFlags::EQ_CHANGE);
            copy
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document_is_top_and_bottom() {
        let doc = Document::new();
        assert_eq!(doc.last_line(), 2);
        assert_eq!(doc.last_real(), 0);
        assert!(doc.line(1).unwrap().is_top());
        assert!(doc.line(2).unwrap().is_bottom());
        assert_eq!(doc.to_text(), "");
    }

    #[test]
    fn load_roundtrip_preserves_newline_style() {
        let doc = Document::from_text("alpha\r\nbeta\r\n");
        assert_eq!(doc.last_real(), 2);
        assert_eq!(doc.line_text(2).unwrap(), "alpha");
        assert_eq!(doc.line_text(3).unwrap(), "beta");
        assert_eq!(doc.line(2).unwrap().seq, 1);
        assert_eq!(doc.line(3).unwrap().seq, 2);
        assert!(doc.line(4).unwrap().is_bottom());
        assert_eq!(doc.to_text(), "alpha\r\nbeta\r\n");
        assert!(!doc.is_modified());
    }

    #[test]
    fn file_without_trailing_newline_stays_that_way() {
        let doc = Document::from_text("alpha\nbeta");
        assert_eq!(doc.to_text(), "alpha\nbeta");
    }

    #[test]
    fn insert_delete_and_undo() {
        let mut doc = Document::from_text("alpha\nbeta\n");
        let first = doc.insert_lines_after(2, 2, true).unwrap();
        assert_eq!(first, 3);
        assert!(doc.line(3).unwrap().is_insert());
        assert_eq!(doc.line_text(4).unwrap(), "");
        assert_eq!(doc.line_text(5).unwrap(), "beta");
        assert!(doc.is_modified());
        doc.set_line_text(3, "middle").unwrap();
        assert!(!doc.line(3).unwrap().is_insert());
        doc.delete_units(3, 2).unwrap();
        assert_eq!(doc.to_text(), "alpha\nbeta\n");
        doc.undo().unwrap();
        assert_eq!(doc.line_text(3).unwrap(), "middle");
        doc.undo().unwrap();
        doc.undo().unwrap();
        assert_eq!(doc.to_text(), "alpha\nbeta\n");
        assert!(!doc.is_modified());
        doc.redo().unwrap();
        assert_eq!(doc.last_real(), 4);
    }

    #[test]
    fn sequence_labels_and_raw_pointers() {
        let mut doc = Document::from_text("one\ntwo\nthree\n");
        doc.set_label(3, Some(".start")).unwrap();
        assert_eq!(doc.resolve_line(".START").unwrap(), 3);
        assert_eq!(doc.resolve_line(".start").unwrap(), 3);
        assert_eq!(doc.resolve_line(".StArT").unwrap(), 3);
        assert_eq!(doc.resolve_line(".ZF").unwrap(), 2);
        assert_eq!(doc.resolve_line(".ZL").unwrap(), 4);
        assert_eq!(doc.resolve_line("2").unwrap(), 3);
        assert_eq!(doc.resolve_line("!4").unwrap(), 4);
        assert_eq!(doc.resolve_line("0").unwrap(), 1);
        assert!(doc.resolve_line(".missing").is_err());
        doc.set_label(4, Some(".start")).unwrap();
        assert!(doc.line(3).unwrap().label.is_none());
        assert_eq!(doc.line(4).unwrap().label.as_deref(), Some(".START"));
    }

    #[test]
    fn min_len_pads_and_char_edits_keep_columns() {
        let mut doc = Document::from_text("ab\n");
        doc.set_min_len(4);
        assert_eq!(doc.line_text(2).unwrap(), "ab  ");
        doc.insert_char(2, 2, 'X').unwrap();
        assert_eq!(doc.line_text(2).unwrap(), "aXb  ");
        doc.delete_char(2, 2).unwrap();
        assert_eq!(doc.line_text(2).unwrap(), "ab  ");
    }

    #[test]
    fn cannot_delete_sentinels() {
        let mut doc = Document::from_text("ab\n");
        assert!(doc.delete_range(1, 2).is_err());
        assert!(doc.delete_range(2, 3).is_err());
    }
}
