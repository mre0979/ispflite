// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyboard and pointer state for the editor window.
//!
//! The screen asks this type what to do with a key. The document still owns
//! the text, so the same path can be tested without opening a window.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::commands::{apply_prefixes, execute, reopen_insert};
use crate::document::{Document, MAX_COLUMN};
use crate::error::EditResult;
use crate::session::Session;

fn empty_file_status(enter_label: &str) -> String {
    format!("Empty file. Type I on Top of Data, then press {enter_label}.")
}

/// Where Enter leaves the cursor when it moves to the next line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextLineCursor {
    LineNumber,
    Text,
}

/// Where typed characters go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Data,
    Prefix,
    Command,
}

/// A mouse click in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerTarget {
    Data { line: usize, col: usize },
    Prefix { line: usize, col: usize },
    Command { col: usize },
}

/// F7 up, F8 down, F10 left, F11 right. The amount is the cursor position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

/// Keys the window translates from the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKey {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Top,
    Bottom,
    PageUp(usize),
    PageDown(usize),
    Enter,
    Tab,
    BackTab,
    Undo,
    Redo,
    Save,
    ToggleInsert,
    Escape,
}

/// One editing session as the window sees it.
pub struct EditorView {
    pub session: Session,
    command: String,
    command_col: usize,
    /// Visible columns of the command field, and the first one currently shown.
    command_field: usize,
    command_scroll: usize,
    status: String,
    /// Typed prefix text, kept per tab so switching tabs does not mix them.
    prefixes: HashMap<u32, BTreeMap<usize, String>>,
    focus: Focus,
    /// Editor focus to restore when the arrow leaves the command line.
    parked_focus: Focus,
    parked_prefix: usize,
    prefix_col: usize,
    insert_mode: bool,
    follow_cursor: bool,
    /// A successful LOCATE should sit on the second editor row.
    scroll_second: bool,
    history: Vec<String>,
    /// Index into `history` while F12 is walking previous commands.
    history_at: Option<usize>,
    /// Anchor of a Shift+arrow selection. The cursor is the other end.
    mark: Mark,
    /// End-key truncation that is still hidden. Tab, line, first column to remove.
    end_cuts: HashMap<u32, HashMap<usize, usize>>,
    /// Undo-stack length at the last Enter, per tab. Escape returns to it.
    committed_undo: HashMap<u32, usize>,
    /// Character columns from the command line's left edge to the editor's left edge.
    editor_shift: isize,
    data_hscroll: usize,
    data_cols: usize,
    /// First visible editor row, and how many rows the screen shows.
    /// A page of 0 means the viewport is unknown, so the caret line stays put.
    screen_top: usize,
    screen_page: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    None,
    Data { line: usize, col: usize },
    Command { col: usize },
}

impl EditorView {
    pub fn new(session: Session) -> Self {
        let empty = session.current().doc.last_real() == 0;
        Self {
            session,
            command: String::new(),
            command_col: 1,
            command_field: 80,
            command_scroll: 0,
            status: if empty {
                empty_file_status("Enter key")
            } else {
                "Ready".to_string()
            },
            prefixes: HashMap::new(),
            focus: Focus::Data,
            parked_focus: Focus::Data,
            parked_prefix: 1,
            prefix_col: 1,
            insert_mode: true,
            follow_cursor: true,
            scroll_second: false,
            history: Vec::new(),
            history_at: None,
            mark: Mark::None,
            end_cuts: HashMap::new(),
            committed_undo: HashMap::new(),
            editor_shift: 0,
            data_hscroll: 0,
            data_cols: 80,
            screen_top: 0,
            screen_page: 0,
        }
    }

    pub fn with_text(text: &str) -> Self {
        let mut session = Session::new();
        session.current_mut().doc = Document::from_text(text);
        Self::new(session)
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn command_col(&self) -> usize {
        self.command_col
    }

    /// The command field is wider than the characters typed into it. The caret
    /// can sit on those blank columns, and a long command scrolls inside it.
    /// Where the editor sits relative to the command line, in character columns.
    pub fn set_cross_layout(&mut self, editor_shift: isize, hscroll: usize, data_cols: usize) {
        self.editor_shift = editor_shift;
        self.data_hscroll = hscroll;
        self.data_cols = data_cols.max(1);
    }

    pub fn set_command_field_width(&mut self, cols: usize) {
        self.command_field = cols.max(1);
        let max = self.command_max_col();
        if self.command_col > max {
            self.command_col = max;
        }
        self.reveal_command_col();
    }

    pub fn command_scroll(&self) -> usize {
        self.command_scroll
    }

    fn command_max_col(&self) -> usize {
        let end = self.command.chars().count() + 1;
        end.max(self.command_field.max(1))
    }

    fn place_command_col(&mut self, col: usize) {
        self.command_col = col.clamp(1, self.command_max_col());
        self.reveal_command_col();
    }

    fn reveal_command_col(&mut self) {
        let width = self.command_field.max(1);
        let index = self.command_col.saturating_sub(1);
        if index < self.command_scroll {
            self.command_scroll = index;
        } else if index >= self.command_scroll + width {
            self.command_scroll = index + 1 - width;
        }
    }

    /// Type at the caret. Insert shifts the rest right. Overlay replaces the
    /// character already there. A caret past the typed text is padded with
    /// spaces so the character lands in that column.
    fn insert_command_char(&mut self, ch: char) {
        let mut chars: Vec<char> = self.command.chars().collect();
        let at = self.command_col.saturating_sub(1);
        if !self.insert_mode && at < chars.len() {
            chars[at] = ch;
            self.command = chars.into_iter().collect();
            self.command_col = at + 2;
            self.reveal_command_col();
            return;
        }
        if at > chars.len() {
            chars.resize(at, ' ');
        }
        let at = at.min(chars.len());
        chars.insert(at, ch);
        self.command = chars.into_iter().collect();
        self.command_col = at + 2;
        self.reveal_command_col();
    }

    pub fn prefix_col(&self) -> usize {
        self.prefix_col
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn insert_mode(&self) -> bool {
        self.insert_mode
    }

    pub fn cursor(&self) -> (usize, usize) {
        self.doc().cursor()
    }

    /// Prefix to paint. A typed command replaces the line number until Enter.
    /// A pending `C`, `CC`, `M`, `MM`, `A`, `B`, `DD`, or `XX` stays visible.
    pub fn shown_prefix(&self, line: usize, fallback: &str) -> String {
        if let Some(text) = self.typed_prefix(line).filter(|text| !text.is_empty()) {
            return pad8(text);
        }
        if let Some(mark) = self.doc().pending().prefix_text(line) {
            return pad8(&mark);
        }
        pad8(fallback)
    }

    pub fn prefix_pending(&self, line: usize) -> bool {
        self.typed_prefix(line).is_some_and(|text| !text.is_empty())
            || self.doc().pending().prefix_text(line).is_some()
    }

    fn typed_prefix(&self, line: usize) -> Option<&str> {
        self.prefixes
            .get(&self.session.current().id)
            .and_then(|map| map.get(&line))
            .map(String::as_str)
    }

    /// The one mark of a command that is still waiting for its partner.
    fn single_pending_text(&self, line: usize) -> Option<String> {
        use crate::document::PendingBlock;
        match self.doc().pending() {
            PendingBlock::CopySpan { .. }
            | PendingBlock::MoveSpan { .. }
            | PendingBlock::DestSpan { .. } => None,
            other => other.prefix_text(line),
        }
    }

    /// Copy a lone pending mark into the prefix so Delete and overlay can edit it.
    fn materialize_single_pending(&mut self, line: usize) {
        let Some(mark) = self.single_pending_text(line) else {
            return;
        };
        let id = self.session.current().id;
        self.prefixes
            .entry(id)
            .or_default()
            .entry(line)
            .or_insert(mark);
    }

    /// A changed or emptied lone mark is no longer the pending command.
    fn finish_prefix_edit(&mut self, line: usize) {
        let typed = self.typed_prefix(line).unwrap_or("").trim().to_string();
        let mark = self.single_pending_text(line);
        if mark.is_none() {
            return;
        }
        if typed.is_empty() {
            let id = self.session.current().id;
            if let Some(map) = self.prefixes.get_mut(&id) {
                map.remove(&line);
            }
        }
        if mark.as_deref() != Some(typed.as_str()) {
            self.doc_mut()
                .set_pending(crate::document::PendingBlock::None);
        }
    }

    pub fn take_follow(&mut self) -> bool {
        std::mem::take(&mut self.follow_cursor)
    }

    /// LOCATE asks for its line on the second editor row. One paint consumes it.
    pub fn take_scroll_second(&mut self) -> bool {
        std::mem::take(&mut self.scroll_second)
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
    }

    /// Keep the empty-file hint in step with the configured Enter key name.
    pub fn refresh_empty_status(&mut self, enter_label: &str) {
        if self.session.current().doc.last_real() != 0 {
            return;
        }
        if self.status.starts_with("Empty file.") || self.status == "Ready" {
            self.status = empty_file_status(enter_label);
        }
    }

    pub fn take_pending_close(&mut self) -> Option<crate::session::PendingClose> {
        self.session.take_pending_close()
    }

    /// F5 and F6 run a primary command and leave the cursor where it landed.
    pub fn run_primary(&mut self, command: &str) {
        self.apply_end_cuts();
        self.arm_screen_find();
        let ran = execute(&mut self.session, command);
        self.doc_mut().set_screen_find_line(None);
        match ran {
            Ok(message) => {
                self.restore_line_numbers();
                self.land_on_number_column();
                self.status = if message.is_empty() {
                    "Ok".to_string()
                } else {
                    message
                };
            }
            Err(err) => self.status = err.message,
        }
        self.commit_input();
    }

    /// F9 swaps. A positive number on the command line chooses the screen and is removed when the swap succeeds.
    pub fn swap_key(&mut self) {
        let screen = screen_number(self.command.trim());
        let owned = screen.map(|n| format!("SWAP {n}"));
        let command = owned.as_deref().unwrap_or("SWAP");
        self.apply_end_cuts();
        self.arm_screen_find();
        let ran = execute(&mut self.session, command);
        self.doc_mut().set_screen_find_line(None);
        match ran {
            Ok(message) => {
                if screen.is_some() {
                    self.command.clear();
                    self.command_col = 1;
                }
                self.status = message;
            }
            Err(err) => self.reject_primary(err.message),
        }
        self.commit_input();
    }

    /// F12 walks back through commands. Shift+F12 walks forward.
    pub fn retrieve(&mut self, forward: bool) {
        if self.history.is_empty() {
            self.status = "No commands to retrieve".to_string();
            return;
        }
        let next = match (self.history_at, forward) {
            (None, false) => self.history.len() - 1,
            (None, true) => {
                self.status = "No further commands".to_string();
                return;
            }
            (Some(0), false) => {
                self.status = "No further commands".to_string();
                return;
            }
            (Some(index), false) => index - 1,
            (Some(index), true) if index + 1 < self.history.len() => index + 1,
            (Some(_), true) => {
                self.history_at = None;
                self.command.clear();
                self.command_col = 1;
                self.remember_editor();
                self.focus = Focus::Command;
                self.status = "Retrieved".to_string();
                return;
            }
        };
        self.history_at = Some(next);
        self.command = self.history[next].clone();
        self.command_col = self.command.chars().count() + 1;
        self.focus = Focus::Command;
        self.status = "Retrieved".to_string();
    }

    /// F3 closes this tab. The last tab is left for the window to close.
    pub fn drop_tab(&mut self) {
        let id = self.session.current().id;
        self.prefixes.remove(&id);
        if self.session.close_current() {
            self.committed_undo.remove(&id);
            self.command.clear();
            self.command_col = 1;
            self.focus = Focus::Data;
            self.follow_cursor = true;
            self.status = "Ended".to_string();
        }
    }

    /// `M` or `MAX` typed on the command line is a one-time scroll amount.
    /// F7, F8, F10, and F11 consume it; any other command is left in place.
    pub fn take_max_scroll(&mut self) -> bool {
        let word = self.command.trim();
        if word.eq_ignore_ascii_case("M") || word.eq_ignore_ascii_case("MAX") {
            self.command.clear();
            self.command_col = 1;
            self.command_scroll = 0;
            true
        } else {
            false
        }
    }

    /// A positive number on the command line is a one-time scroll amount.
    /// F7, F8, F10, and F11 consume it; any other command is left in place.
    pub fn take_line_scroll(&mut self) -> Option<usize> {
        let word = self.command.trim();
        if word.is_empty() || !word.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let lines = word.parse::<usize>().ok().filter(|lines| *lines > 0)?;
        self.command.clear();
        self.command_col = 1;
        self.command_scroll = 0;
        Some(lines)
    }

    /// New top row after scrolling `lines` up or down, clamped to the file.
    /// Sets the status to the edge name when the scroll lands on one.
    pub fn scroll_by_lines(&mut self, down: bool, lines: usize, top: usize, page: usize) -> usize {
        let page = page.max(1);
        let count = self.doc().visible_rows().len();
        let max_top = count.saturating_sub(page);
        let top = top.min(max_top);
        let new_top = if down {
            top.saturating_add(lines).min(max_top)
        } else {
            top.saturating_sub(lines)
        };
        self.status = if down && new_top >= max_top {
            "Bottom of data".to_string()
        } else if !down && new_top == 0 {
            "Top of data".to_string()
        } else if down {
            format!("Down {lines}")
        } else {
            format!("Up {lines}")
        };
        new_top
    }

    /// New left column after scrolling `cols` left or right. Column 1 is the left edge.
    pub fn scroll_by_columns(&mut self, right: bool, cols: usize, left: usize) -> usize {
        let new_left = if right {
            left.saturating_add(cols)
        } else {
            left.saturating_sub(cols)
        };
        self.status = if !right && new_left == 0 {
            "Left of data".to_string()
        } else if right {
            format!("Right {cols}")
        } else {
            format!("Left {cols}")
        };
        new_left
    }

    /// After F10, a caret the scroll left off the screen moves to the rightmost
    /// visible column. A caret already on the screen stays where it is.
    pub fn land_on_rightmost_if_outside(&mut self, left: usize, screen_cols: usize) {
        let screen_cols = screen_cols.max(1);
        let (line, col) = self.cursor();
        let col0 = col.saturating_sub(1);
        if col0 >= left && col0 < left + screen_cols {
            return;
        }
        let _ = self.doc_mut().set_cursor(line, left + screen_cols);
    }

    /// F10 with `M` goes to column 1. F11 with `M` goes to the end of the
    /// longest line, with that column at the right edge when it is past the
    /// screen. The caret stays on the command line. Returns the new left column.
    pub fn scroll_max_columns(&mut self, right: bool, screen_cols: usize) -> usize {
        let screen_cols = screen_cols.max(1);
        let width = self.doc().max_length().clamp(1, MAX_COLUMN);
        let left = if right {
            width.saturating_sub(screen_cols)
        } else {
            0
        };
        let col = if right { width } else { 1 };
        let line = self.cursor().0;
        let _ = self.doc_mut().set_cursor(line, col);
        self.focus = Focus::Data;
        self.remember_editor();
        self.focus = Focus::Command;
        self.status = if right {
            "Right of data".to_string()
        } else {
            "Left of data".to_string()
        };
        left
    }

    /// F7 with `M` goes to the top. F8 with `M` goes to the bottom.
    /// The caret stays on the command line.
    pub fn scroll_max(&mut self, top: bool) {
        self.jump_edge(top);
        self.remember_editor();
        self.focus = Focus::Command;
        self.status = if top {
            "Top of data".to_string()
        } else {
            "Bottom of data".to_string()
        };
    }

    /// Alt+Left and Alt+Right move to the previous or next non-space character
    /// on this line. Shift extends the selection.
    pub fn jump_nonspace(&mut self, forward: bool, extend: bool) {
        if self.focus == Focus::Prefix {
            self.focus = Focus::Data;
        }
        if extend {
            self.ensure_anchor();
        } else {
            self.mark = Mark::None;
        }
        match self.focus {
            Focus::Data => {
                let (line, col) = self.cursor();
                let chars = self
                    .doc()
                    .line(line)
                    .map(|row| row.chars())
                    .unwrap_or_default();
                let next = if forward {
                    next_nonspace_col(&chars, col)
                } else {
                    prev_nonspace_col(&chars, col)
                };
                let _ = self.doc_mut().set_cursor(line, next);
                self.follow_cursor = true;
            }
            Focus::Command => {
                let chars: Vec<char> = self.command.chars().collect();
                self.command_col = if forward {
                    next_nonspace_col(&chars, self.command_col)
                } else {
                    prev_nonspace_col(&chars, self.command_col)
                };
            }
            Focus::Prefix => {}
        }
    }

    /// Shift+arrow extends the selection. An arrow without Shift clears it.
    pub fn nudge(&mut self, key: EditorKey, extend: bool) {
        if extend {
            self.ensure_anchor();
            self.extend_move(key);
        } else {
            self.mark = Mark::None;
            self.handle_key(key);
        }
    }

    /// Columns of `line` covered by the data selection. The end column is exclusive.
    pub fn data_selection(&self, line: usize) -> Option<(usize, usize)> {
        let Mark::Data {
            line: anchor_line,
            col: anchor_col,
        } = self.mark
        else {
            return None;
        };
        if self.focus != Focus::Data {
            return None;
        }
        let (cur_line, cur_col) = self.cursor();
        let ((start_line, start_col), (end_line, end_col)) =
            order_points(anchor_line, anchor_col, cur_line, cur_col);
        if (start_line, start_col) == (end_line, end_col) {
            return None;
        }
        if line < start_line || line > end_line {
            return None;
        }
        let len = self.doc().line(line).map(|row| row.char_len()).unwrap_or(0);
        if start_line == end_line {
            return Some((start_col, end_col));
        }
        if line == start_line {
            let end = len + 1;
            if start_col >= end {
                return None;
            }
            return Some((start_col, end));
        }
        if line == end_line {
            if end_col <= 1 {
                return None;
            }
            return Some((1, end_col.min(len + 1)));
        }
        Some((1, (len + 1).max(2)))
    }

    /// Columns of the command line covered by the selection. The end is exclusive.
    pub fn command_selection(&self) -> Option<(usize, usize)> {
        let Mark::Command { col: anchor } = self.mark else {
            return None;
        };
        if self.focus != Focus::Command {
            return None;
        }
        if anchor == self.command_col {
            return None;
        }
        if anchor < self.command_col {
            Some((anchor, self.command_col))
        } else {
            Some((self.command_col, anchor))
        }
    }

    /// Select every data line, from the first character through the end of the last line.
    pub fn select_all(&mut self) {
        let lines: Vec<usize> = (1..=self.doc().last_line())
            .filter(|&line| self.doc().line(line).is_ok_and(|row| row.is_data()))
            .collect();
        let Some(&start) = lines.first() else {
            self.mark = Mark::None;
            self.status = "No text selected".to_string();
            return;
        };
        let end = *lines.last().unwrap_or(&start);
        let end_col = self.doc().line(end).map(|row| row.char_len()).unwrap_or(0) + 1;
        self.focus = Focus::Data;
        self.mark = Mark::Data {
            line: start,
            col: 1,
        };
        let _ = self.doc_mut().set_cursor(end, end_col.max(1));
        self.follow_cursor = true;
        self.status = "Selected all".to_string();
    }

    pub fn copy_selection(&mut self) -> Option<String> {
        let Some(text) = self.selected_text() else {
            self.status = "No text selected".to_string();
            return None;
        };
        self.status = "Copied".to_string();
        Some(text)
    }

    pub fn cut_selection(&mut self) -> Option<String> {
        if self.selected_text().is_none() {
            self.status = "No text selected".to_string();
            return None;
        }
        let text = self.selected_text()?;
        let blank = !self.insert_mode && self.focus == Focus::Data;
        if blank {
            if !self.blank_selection() {
                return None;
            }
        } else if !self.delete_selection() {
            return None;
        }
        self.status = "Cut".to_string();
        Some(text)
    }

    pub fn paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let overlay = !self.insert_mode && self.focus == Focus::Data;
        if self.has_selection() {
            let cleared = if overlay {
                self.blank_selection()
            } else {
                self.delete_selection()
            };
            if !cleared {
                return;
            }
        }
        match self.focus {
            Focus::Data if overlay => match self.overlay_paste(text) {
                Ok(()) => {
                    self.follow_cursor = true;
                    self.status = "Pasted".to_string();
                }
                Err(err) => self.status = err.message,
            },
            Focus::Data => {
                let (line, col) = self.cursor();
                let pending = self.pending_end(line);
                let splice_at = pending.filter(|cut| col > *cut).unwrap_or(col);
                let insert = if splice_at < col {
                    prepend_gap(text, col - splice_at)
                } else {
                    text.to_string()
                };
                match self
                    .doc_mut()
                    .splice_text(line, splice_at, line, splice_at, &insert)
                {
                    Ok(()) => {
                        let _ = self.doc_mut().set_cursor(line, col);
                        self.note_pasted(line, col, pending, &insert);
                        self.follow_cursor = true;
                        self.status = "Pasted".to_string();
                    }
                    Err(err) => self.status = err.message,
                }
            }
            Focus::Command => {
                let start = self.command_col;
                let flat: String = text
                    .chars()
                    .map(|ch| match ch {
                        '\n' | '\r' => ' ',
                        other => other,
                    })
                    .collect();
                for ch in flat.chars() {
                    self.insert_command_char(ch);
                }
                self.place_command_col(start);
                self.status = "Pasted".to_string();
            }
            Focus::Prefix => self.status = "Cannot paste here".to_string(),
        }
    }

    /// Overlay paste. Each character replaces the one under the caret; later text stays put.
    fn overlay_paste(&mut self, text: &str) -> EditResult<()> {
        let (start_line, start_col) = self.cursor();
        let mut line = start_line;
        let mut col = start_col;
        let rows = self.doc().visible_rows();
        let mut row_at = rows.iter().position(|row| row.line == line);
        for ch in text.chars() {
            if ch == '\r' {
                continue;
            }
            if ch == '\n' {
                let Some(index) = row_at else {
                    break;
                };
                let next = index + 1;
                if next >= rows.len() {
                    break;
                }
                line = rows[next].line;
                col = start_col;
                row_at = Some(next);
                continue;
            }
            self.doc_mut().replace_char(line, col, ch)?;
            col = (col + 1).min(MAX_COLUMN);
        }
        let _ = self.doc_mut().set_cursor(start_line, start_col);
        Ok(())
    }

    /// Keep a pending End cut past text that was pasted into the hidden tail.
    fn note_pasted(&mut self, line: usize, col: usize, pending: Option<usize>, insert: &str) {
        let Some(cut) = pending else {
            return;
        };
        let splice_at = if col > cut { cut } else { col };
        let lines: Vec<&str> = insert.split('\n').collect();
        let line_len = |part: &str| part.trim_end_matches('\r').chars().count();
        let first_len = lines.first().copied().map(line_len).unwrap_or(0);
        if lines.len() <= 1 {
            self.set_pending_end(line, cut + first_len);
            return;
        }
        self.clear_pending_end(line);
        let last = line + lines.len() - 1;
        let last_len = lines.last().copied().map(line_len).unwrap_or(0);
        let hidden_at = last_len + cut.saturating_sub(splice_at) + 1;
        self.set_pending_end(last, hidden_at);
    }

    pub fn handle_key(&mut self, key: EditorKey) {
        match key {
            EditorKey::Char(_) | EditorKey::Backspace | EditorKey::Delete => {}
            _ => self.mark = Mark::None,
        }
        match key {
            EditorKey::Char(ch) if !ch.is_control() => self.type_char(ch),
            EditorKey::Backspace => self.backspace(),
            EditorKey::Delete => self.delete(),
            EditorKey::Left => self.horizontal(-1),
            EditorKey::Right => self.horizontal(1),
            EditorKey::Up => self.up(),
            EditorKey::Down => self.down(),
            EditorKey::Home => self.home(),
            EditorKey::End => self.end(),
            EditorKey::Top => self.jump_edge(true),
            EditorKey::Bottom => self.jump_edge(false),
            EditorKey::PageUp(rows) => self.page(-(rows.max(1) as isize)),
            EditorKey::PageDown(rows) => self.page(rows.max(1) as isize),
            EditorKey::Enter => {
                self.attention();
                self.commit_input();
            }
            EditorKey::Escape => self.discard_pending(),
            EditorKey::Tab => self.cycle_focus(1),
            EditorKey::BackTab => self.cycle_focus(-1),
            EditorKey::Undo => self.undo(true),
            EditorKey::Redo => self.undo(false),
            EditorKey::Save => {
                self.save();
            }
            EditorKey::ToggleInsert => {
                self.insert_mode = !self.insert_mode;
                self.status = if self.insert_mode {
                    "Insert".to_string()
                } else {
                    "Overtype".to_string()
                };
            }
            EditorKey::Char(_) => {}
        }
    }

    pub fn click(&mut self, target: PointerTarget) {
        self.mark = Mark::None;
        match target {
            PointerTarget::Data { line, col } => {
                if self.doc_mut().set_cursor(line, col.max(1)).is_ok() {
                    self.focus = Focus::Data;
                }
            }
            PointerTarget::Prefix { line, col } => {
                let data_col = self.doc().cursor().1;
                if self.doc_mut().set_cursor(line, data_col).is_ok() {
                    self.focus = Focus::Prefix;
                    self.prefix_col = col.clamp(1, 8);
                }
            }
            PointerTarget::Command { col } => {
                self.remember_editor();
                self.focus = Focus::Command;
                self.place_command_col(col);
            }
        }
    }

    /// Move to a line from the COBOL outline and scroll it into view.
    pub fn jump_to_line(&mut self, mut line: usize, col: usize) {
        if self.doc().line(line).is_ok_and(|row| row.is_invisible()) {
            if let Ok(shown) = self.doc_mut().pop_out(line) {
                line = shown;
            }
        }
        self.click(PointerTarget::Data {
            line,
            col: col.max(1),
        });
        self.follow_cursor = true;
    }

    pub fn open_path(&mut self, path: &Path) {
        match self.session.open_path(path) {
            Ok(()) => {
                let id = self.session.current().id;
                self.prefixes.remove(&id);
                self.command.clear();
                self.command_col = 1;
                self.command_scroll = 0;
                self.focus = Focus::Data;
                self.follow_cursor = true;
                self.status = format!("Opened {}", path.display());
                self.commit_input();
            }
            Err(err) => self.status = err.message,
        }
    }

    /// Load the current file again and throw away the buffer.
    pub fn reopen_current(&mut self) -> bool {
        let id = self.session.current().id;
        let Some(path) = self.session.current().path.clone() else {
            self.status = "No file to re-open".to_string();
            return false;
        };
        match self.session.reload_current() {
            Ok(()) => {
                self.prefixes.remove(&id);
                self.end_cuts.remove(&id);
                self.mark = Mark::None;
                self.command.clear();
                self.command_col = 1;
                self.command_scroll = 0;
                self.focus = Focus::Data;
                self.follow_cursor = true;
                self.status = format!("Re-opened {}", path.display());
                self.commit_input();
                true
            }
            Err(err) => {
                self.status = err.message;
                false
            }
        }
    }

    pub fn save(&mut self) -> bool {
        match self.session.save_current(None) {
            Ok(path) => {
                self.status = format!("Saved {}", path.display());
                true
            }
            Err(err) => {
                self.status = err.message;
                false
            }
        }
    }

    pub fn save_as(&mut self, path: &Path) -> bool {
        match self.session.save_current(Some(&path.to_string_lossy())) {
            Ok(path) => {
                self.status = format!("Saved {}", path.display());
                true
            }
            Err(err) => {
                self.status = err.message;
                false
            }
        }
    }

    fn ensure_anchor(&mut self) {
        match self.focus {
            Focus::Data => {
                if !matches!(self.mark, Mark::Data { .. }) {
                    let (line, col) = self.cursor();
                    self.mark = Mark::Data { line, col };
                }
            }
            Focus::Command => {
                if !matches!(self.mark, Mark::Command { .. }) {
                    self.mark = Mark::Command {
                        col: self.command_col,
                    };
                }
            }
            Focus::Prefix => self.mark = Mark::None,
        }
    }

    fn extend_move(&mut self, key: EditorKey) {
        match (self.focus, key) {
            (Focus::Data, EditorKey::Left) => self.extend_data_col(-1),
            (Focus::Data, EditorKey::Right) => self.extend_data_col(1),
            (Focus::Data, EditorKey::Up) => self.move_rows(-1),
            (Focus::Data, EditorKey::Down) => self.move_rows(1),
            (Focus::Command, EditorKey::Left) => {
                self.place_command_col(self.command_col.saturating_sub(1));
            }
            (Focus::Command, EditorKey::Right) => {
                self.place_command_col(self.command_col.saturating_add(1));
            }
            _ => {
                self.mark = Mark::None;
                self.handle_key(key);
            }
        }
    }

    fn extend_data_col(&mut self, delta: isize) {
        let (line, col) = self.cursor();
        if delta > 0 && col >= MAX_COLUMN {
            self.move_to_neighbor(1);
            return;
        }
        if delta < 0 && col <= 1 {
            self.move_to_neighbor(-1);
            return;
        }
        let next = (col as isize + delta).clamp(1, MAX_COLUMN as isize) as usize;
        let _ = self.doc_mut().set_cursor(line, next);
        self.follow_cursor = true;
    }

    fn move_to_neighbor(&mut self, delta: isize) {
        let rows = self.doc().visible_rows();
        if rows.is_empty() {
            return;
        }
        let (line, _) = self.cursor();
        let idx = rows.iter().position(|row| row.line == line).unwrap_or(0);
        let next = idx as isize + delta;
        if next < 0 || next >= rows.len() as isize {
            return;
        }
        let target = rows[next as usize].line;
        let col = if delta > 0 {
            1
        } else {
            self.doc()
                .line(target)
                .map(|row| row.char_len())
                .unwrap_or(0)
                + 1
        };
        let _ = self.doc_mut().set_cursor(target, col);
        self.focus = Focus::Data;
        self.follow_cursor = true;
    }

    fn has_selection(&self) -> bool {
        self.selected_text().is_some()
    }

    fn selected_text(&self) -> Option<String> {
        match self.mark {
            Mark::Data {
                line: anchor_line,
                col: anchor_col,
            } if self.focus == Focus::Data => {
                let (line, col) = self.cursor();
                let text = text_between(self.doc(), anchor_line, anchor_col, line, col);
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            Mark::Command { col: anchor } if self.focus == Focus::Command => {
                let (start, end) = self.command_selection()?;
                let chars: Vec<char> = self.command.chars().collect();
                let start = start.saturating_sub(1).min(chars.len());
                let end = end.saturating_sub(1).min(chars.len());
                let text: String = chars[start..end].iter().collect();
                let _ = anchor;
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            _ => None,
        }
    }

    fn delete_selection(&mut self) -> bool {
        match self.mark {
            Mark::Data { line, col } if self.focus == Focus::Data => {
                let (cur_line, cur_col) = self.cursor();
                if (cur_line, cur_col) == (line, col) {
                    self.mark = Mark::None;
                    return false;
                }
                match self.doc_mut().splice_text(line, col, cur_line, cur_col, "") {
                    Ok(()) => {
                        self.mark = Mark::None;
                        self.follow_cursor = true;
                        true
                    }
                    Err(err) => {
                        self.status = err.message;
                        false
                    }
                }
            }
            Mark::Command { col: anchor } if self.focus == Focus::Command => {
                let Some((start, end)) = self.command_selection() else {
                    self.mark = Mark::None;
                    return false;
                };
                let chars: Vec<char> = self.command.chars().collect();
                let from = start.saturating_sub(1).min(chars.len());
                let to = end.saturating_sub(1).min(chars.len());
                let mut kept = chars[..from].to_vec();
                kept.extend_from_slice(&chars[to..]);
                self.command = kept.into_iter().collect();
                self.command_col = start;
                self.mark = Mark::None;
                let _ = anchor;
                true
            }
            _ => false,
        }
    }

    /// Overlay cut. Replace the selection with spaces so later characters stay put.
    fn blank_selection(&mut self) -> bool {
        let Mark::Data {
            line: anchor_line,
            col: anchor_col,
        } = self.mark
        else {
            return false;
        };
        if self.focus != Focus::Data {
            return false;
        }
        let (cur_line, cur_col) = self.cursor();
        if (cur_line, cur_col) == (anchor_line, anchor_col) {
            self.mark = Mark::None;
            return false;
        }
        let ((start_line, start_col), (end_line, end_col)) =
            order_points(anchor_line, anchor_col, cur_line, cur_col);
        match blank_range(self.doc_mut(), start_line, start_col, end_line, end_col) {
            Ok(()) => {
                self.mark = Mark::None;
                let _ = self.doc_mut().set_cursor(start_line, start_col);
                self.follow_cursor = true;
                true
            }
            Err(err) => {
                self.status = err.message;
                false
            }
        }
    }

    fn type_char(&mut self, ch: char) {
        if self.has_selection() && !self.delete_selection() {
            return;
        }
        match self.focus {
            Focus::Data => {
                let (line, col) = self.cursor();
                if self.pending_end(line).is_some_and(|cut| col >= cut) {
                    self.type_into_erased(line, col, ch);
                    return;
                }
                let len = self.doc().line(line).map(|row| row.char_len()).unwrap_or(0);
                let inserted = self.insert_mode || col > len;
                let result = if inserted {
                    self.doc_mut().insert_char(line, col, ch)
                } else {
                    self.doc_mut().replace_char(line, col, ch)
                };
                match result {
                    Ok(()) => {
                        if inserted || self.pending_end(line) == Some(col) {
                            self.note_inserted(line, col);
                        }
                        let _ = self.doc_mut().set_cursor(line, col + 1);
                        self.status.clear();
                    }
                    Err(err) => self.status = err.message,
                }
            }
            Focus::Prefix => self.type_prefix(ch),
            Focus::Command => {
                self.insert_command_char(ch);
                self.status.clear();
            }
        }
    }

    fn type_prefix(&mut self, ch: char) {
        let line = self.cursor().0;
        let id = self.session.current().id;
        if !self.insert_mode {
            self.materialize_single_pending(line);
        }
        // A space over a label, with no line command typed, is stored until Enter.
        let seeded =
            if ch != ' ' && !self.insert_mode && self.doc().pending().prefix_text(line).is_none() {
                self.doc()
                    .line(line)
                    .ok()
                    .and_then(|row| row.label.clone())
                    .map(|label| pad8(&label))
            } else {
                None
            };
        let map = self.prefixes.entry(id).or_default();
        let fresh = !map.contains_key(&line);
        let entry = map.entry(line).or_default();
        if fresh {
            if let Some(label) = seeded {
                *entry = label;
            }
        }
        let len = entry.chars().count();
        if !self.insert_mode && self.prefix_col <= len {
            replace_at(entry, self.prefix_col, ch);
            self.prefix_col = (self.prefix_col + 1).min(9);
        } else if entry.is_empty() {
            entry.push(ch);
            self.prefix_col = 2;
        } else {
            insert_at(entry, &mut self.prefix_col, ch);
            if entry.chars().count() > 8 {
                *entry = entry.chars().take(8).collect();
                self.prefix_col = self.prefix_col.min(9);
            }
        }
        self.finish_prefix_edit(line);
        self.status.clear();
    }

    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        match self.focus {
            Focus::Data => {
                let (line, col) = self.cursor();
                if col <= 1 {
                    return;
                }
                if self.pending_end(line).is_some_and(|cut| col > cut) {
                    let _ = self.doc_mut().set_cursor(line, col - 1);
                    self.follow_cursor = true;
                    return;
                }
                let len = self.doc().line(line).map(|row| row.char_len()).unwrap_or(0);
                if col > len + 1 {
                    let _ = self.doc_mut().set_cursor(line, col - 1);
                    self.follow_cursor = true;
                    return;
                }
                if self.doc_mut().delete_char(line, col - 1).is_ok() {
                    self.note_removed(line, col - 1);
                    let _ = self.doc_mut().set_cursor(line, col - 1);
                    self.status.clear();
                }
            }
            Focus::Prefix => self.prefix_backspace(),
            Focus::Command => {
                let len = self.command.chars().count();
                if self.command_col > len + 1 {
                    self.place_command_col(self.command_col - 1);
                } else {
                    delete_before(&mut self.command, &mut self.command_col);
                    self.reveal_command_col();
                }
            }
        }
    }

    fn prefix_backspace(&mut self) {
        let line = self.cursor().0;
        let id = self.session.current().id;
        let Some(map) = self.prefixes.get_mut(&id) else {
            return;
        };
        let Some(entry) = map.get_mut(&line) else {
            return;
        };
        delete_before(entry, &mut self.prefix_col);
        if entry.is_empty() {
            map.remove(&line);
            self.prefix_col = 1;
        }
    }

    fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        match self.focus {
            Focus::Data => {
                let (line, col) = self.cursor();
                if self.pending_end(line).is_some_and(|cut| col >= cut) {
                    return;
                }
                if self.doc_mut().delete_char(line, col).is_ok() {
                    self.note_removed(line, col);
                    self.status.clear();
                }
            }
            Focus::Prefix => {
                let line = self.cursor().0;
                self.materialize_single_pending(line);
                let id = self.session.current().id;
                if let Some(entry) = self
                    .prefixes
                    .get_mut(&id)
                    .and_then(|map| map.get_mut(&line))
                {
                    delete_at(entry, self.prefix_col);
                    if entry.is_empty() {
                        self.prefixes.get_mut(&id).unwrap().remove(&line);
                    }
                }
                self.finish_prefix_edit(line);
            }
            Focus::Command => delete_at(&mut self.command, self.command_col),
        }
    }

    fn horizontal(&mut self, delta: isize) {
        match self.focus {
            Focus::Data => {
                let (line, col) = self.cursor();
                if delta < 0 && col <= 1 {
                    self.focus = Focus::Prefix;
                    self.prefix_col = 8;
                    return;
                }
                let next = (col as isize + delta).clamp(1, MAX_COLUMN as isize) as usize;
                let _ = self.doc_mut().set_cursor(line, next);
                self.follow_cursor = true;
            }
            Focus::Prefix => {
                let next = self.prefix_col as isize + delta;
                if delta > 0 && next > 8 {
                    let line = self.cursor().0;
                    self.focus = Focus::Data;
                    let _ = self.doc_mut().set_cursor(line, 1);
                    return;
                }
                if next >= 1 {
                    self.prefix_col = (next as usize).min(8);
                }
            }
            Focus::Command => {
                let next = (self.command_col as isize + delta).max(1) as usize;
                self.place_command_col(next);
            }
        }
    }

    fn up(&mut self) {
        if self.focus == Focus::Command {
            return;
        }
        let rows = self.doc().visible_rows();
        let (line, _) = self.cursor();
        let at_top = rows.first().is_some_and(|row| row.line == line);
        if at_top {
            let screen = self.screen_col();
            self.remember_editor();
            self.focus = Focus::Command;
            self.place_nearest_command(screen);
            self.follow_cursor = true;
            return;
        }
        let stay_prefix = self.focus == Focus::Prefix;
        self.move_rows(-1);
        if stay_prefix {
            self.focus = Focus::Prefix;
        }
    }

    fn down(&mut self) {
        if self.focus == Focus::Command {
            self.land_on_shown_line();
            self.focus = if self.parked_focus == Focus::Prefix {
                Focus::Prefix
            } else {
                Focus::Data
            };
            self.prefix_col = self.parked_prefix.clamp(1, 8);
            self.follow_cursor = true;
            return;
        }
        let stay_prefix = self.focus == Focus::Prefix;
        self.move_rows(1);
        if stay_prefix {
            self.focus = Focus::Prefix;
        }
    }

    /// Enter, when a Ctrl key is the attention key, moves onto the next line.
    /// From the command line it lands on the first line currently on screen.
    pub fn next_line(&mut self, at: NextLineCursor, first_visible: usize) {
        if self.focus == Focus::Command {
            self.land_on_screen_line(first_visible, at);
            return;
        }
        self.move_rows(1);
        if at == NextLineCursor::LineNumber {
            self.focus = Focus::Prefix;
            self.prefix_col = 1;
        }
    }

    fn land_on_screen_line(&mut self, index: usize, at: NextLineCursor) {
        let rows = self.doc().visible_rows();
        let Some(row) = rows.get(index).or_else(|| rows.last()) else {
            return;
        };
        let col = self.cursor().1;
        let _ = self.doc_mut().set_cursor(row.line, col);
        self.follow_cursor = true;
        if at == NextLineCursor::LineNumber {
            self.focus = Focus::Prefix;
            self.prefix_col = 1;
        } else {
            self.focus = Focus::Data;
        }
    }

    /// Column of the caret, counting from the left edge of the command line.
    fn screen_col(&self) -> isize {
        match self.focus {
            Focus::Command => 9 + self.command_col as isize - 1 - self.command_scroll as isize,
            Focus::Prefix => self.editor_shift + self.prefix_col as isize - 1,
            Focus::Data => {
                self.editor_shift + 9 + self.cursor().1 as isize - 1 - self.data_hscroll as isize
            }
        }
    }

    /// Command-field cell closest to `screen`. The prompt occupies the first 9 columns.
    fn place_nearest_command(&mut self, screen: isize) {
        let width = self.command_field.max(1) as isize;
        let left = 9;
        let slot = screen.clamp(left, left + width - 1);
        let col = self.command_scroll as isize + (slot - left) + 1;
        self.place_command_col(col.max(1) as usize);
    }

    /// Prefix or data cell closest to `screen`. The prefix and the gap are 9 columns.
    fn place_nearest_editor(&mut self, screen: isize) {
        let line = self.cursor().0;
        let origin = self.editor_shift;
        if screen < origin + 8 {
            let slot = screen.clamp(origin, origin + 7);
            let col = (slot - origin) as usize + 1;
            self.click(PointerTarget::Prefix { line, col });
        } else {
            let data_left = origin + 9;
            let data_right = data_left + self.data_cols.max(1) as isize - 1;
            let slot = screen.clamp(data_left, data_right);
            let col = self.data_hscroll as isize + (slot - data_left) + 1;
            self.click(PointerTarget::Data {
                line,
                col: col.max(1) as usize,
            });
        }
    }

    fn home(&mut self) {
        self.remember_editor();
        self.focus = Focus::Command;
        self.command_col = 1;
    }

    /// Keep the editor caret so leaving the command line can return to it.
    fn remember_editor(&mut self) {
        if self.focus != Focus::Command {
            self.parked_focus = self.focus;
            self.parked_prefix = self.prefix_col;
        }
    }

    fn end(&mut self) {
        match self.focus {
            Focus::Data => self.mark_end_of_line(),
            Focus::Prefix => self.clear_prefix_tail(),
            Focus::Command => self.clear_command_tail(),
        }
    }

    /// Drop the typed line command from the caret through the end of the field.
    /// An untouched line number is left alone.
    fn clear_prefix_tail(&mut self) {
        let line = self.cursor().0;
        let keep = self.prefix_col.saturating_sub(1);
        let id = self.session.current().id;
        let Some(map) = self.prefixes.get_mut(&id) else {
            return;
        };
        let Some(entry) = map.get(&line) else {
            return;
        };
        if keep >= entry.chars().count() {
            return;
        }
        let next: String = entry.chars().take(keep).collect();
        if next.is_empty() {
            map.remove(&line);
            self.prefix_col = 1;
        } else {
            map.insert(line, next);
            self.prefix_col = keep + 1;
        }
        self.status = "Deleted to end of line".to_string();
    }

    /// Drop the command from the caret through the end of the typed text.
    fn clear_command_tail(&mut self) {
        let keep = self.command_col.saturating_sub(1);
        if keep >= self.command.chars().count() {
            return;
        }
        self.command = self.command.chars().take(keep).collect();
        self.command_col = keep + 1;
        self.reveal_command_col();
        self.status = "Deleted to end of line".to_string();
    }

    /// Hide from the caret through the end of the data line. The text stays
    /// until Enter, a line command, or a primary command. A second End inside
    /// the columns already hidden leaves the caret where it is.
    fn mark_end_of_line(&mut self) {
        let (line, col) = self.cursor();
        if self.pending_end(line).is_some_and(|cut| col >= cut) {
            return;
        }
        let Ok(row) = self.doc().line(line) else {
            return;
        };
        if !row.is_data() && !row.is_note() {
            return;
        }
        if col > row.char_len() {
            return;
        }
        self.set_pending_end(line, col);
        let _ = self.doc_mut().set_cursor(line, col);
        self.status = "Pending end of line".to_string();
    }

    pub fn pending_end(&self, line: usize) -> Option<usize> {
        let id = self.session.current().id;
        self.end_cuts.get(&id)?.get(&line).copied()
    }

    /// Text with a pending End cut removed, for the screen only.
    pub fn shown_line<'a>(&self, line: usize, text: &'a str) -> std::borrow::Cow<'a, str> {
        match self.pending_end(line) {
            Some(col) => {
                std::borrow::Cow::Owned(text.chars().take(col.saturating_sub(1)).collect())
            }
            None => std::borrow::Cow::Borrowed(text),
        }
    }

    pub fn shown_find_spans(&self, line: usize) -> Vec<(usize, usize)> {
        let spans = self.doc().find_spans(line);
        let Some(cut) = self.pending_end(line) else {
            return spans;
        };
        spans
            .into_iter()
            .filter_map(|(col, len)| {
                if col >= cut || len == 0 {
                    None
                } else {
                    Some((col, len.min(cut - col)))
                }
            })
            .collect()
    }

    fn clear_pending_end(&mut self, line: usize) {
        let id = self.session.current().id;
        if let Some(map) = self.end_cuts.get_mut(&id) {
            map.remove(&line);
        }
    }

    /// Type into the blank columns End is hiding. The old tail stays hidden.
    fn type_into_erased(&mut self, line: usize, col: usize, ch: char) {
        let Some(cut) = self.pending_end(line) else {
            return;
        };
        let gap = col.saturating_sub(cut);
        let mut insert = " ".repeat(gap);
        insert.push(ch);
        match self.doc_mut().splice_text(line, cut, line, cut, &insert) {
            Ok(()) => {
                self.set_pending_end(line, cut + insert.chars().count());
                let _ = self.doc_mut().set_cursor(line, col + 1);
                self.status.clear();
            }
            Err(err) => self.status = err.message,
        }
    }

    fn set_pending_end(&mut self, line: usize, col: usize) {
        let id = self.session.current().id;
        self.end_cuts.entry(id).or_default().insert(line, col);
    }

    fn note_inserted(&mut self, line: usize, col: usize) {
        let id = self.session.current().id;
        if let Some(cut) = self
            .end_cuts
            .get_mut(&id)
            .and_then(|map| map.get_mut(&line))
        {
            if col <= *cut {
                *cut += 1;
            }
        }
    }

    fn note_removed(&mut self, line: usize, col: usize) {
        let id = self.session.current().id;
        if let Some(cut) = self
            .end_cuts
            .get_mut(&id)
            .and_then(|map| map.get_mut(&line))
        {
            if col < *cut {
                *cut = cut.saturating_sub(1).max(1);
            }
        }
    }

    /// Write pending End cuts into the file. Later commands see the shorter lines.
    fn apply_end_cuts(&mut self) -> bool {
        let id = self.session.current().id;
        let Some(map) = self.end_cuts.remove(&id) else {
            return false;
        };
        let mut changed = false;
        for (line, col) in map {
            if self
                .doc_mut()
                .delete_from_column(line, col)
                .unwrap_or(false)
            {
                changed = true;
            }
        }
        if changed {
            self.status = "Deleted to end of line".to_string();
        }
        changed
    }

    fn jump_edge(&mut self, top: bool) {
        let rows = self.doc().visible_rows();
        let Some(row) = (if top { rows.first() } else { rows.last() }) else {
            return;
        };
        let line = row.line;
        let col = self.cursor().1;
        let _ = self.doc_mut().set_cursor(line, col);
        self.focus = Focus::Data;
        self.follow_cursor = true;
    }

    fn page(&mut self, delta: isize) {
        if self.focus == Focus::Command {
            self.focus = Focus::Data;
        }
        self.move_rows(delta);
    }

    fn move_rows(&mut self, delta: isize) {
        let rows = self.doc().visible_rows();
        if rows.is_empty() {
            return;
        }
        let (line, col) = self.cursor();
        let idx = rows.iter().position(|row| row.line == line).unwrap_or(0);
        let last = rows.len() - 1;
        let next = (idx as isize + delta).clamp(0, last as isize) as usize;
        let target = rows[next].line;
        let _ = self.doc_mut().set_cursor(target, col);
        self.focus = Focus::Data;
        self.follow_cursor = true;
    }

    fn cycle_focus(&mut self, dir: isize) {
        let from_command = self.focus == Focus::Command;
        let next = match (self.focus, dir) {
            (Focus::Data, 1) | (Focus::Prefix, -1) => Focus::Command,
            (Focus::Command, 1) | (Focus::Data, -1) => Focus::Prefix,
            (Focus::Prefix, _) | (Focus::Command, _) | (Focus::Data, _) => Focus::Data,
        };
        if next == Focus::Command {
            self.remember_editor();
        }
        self.focus = next;
        if from_command && next != Focus::Command {
            self.land_on_shown_line();
            self.follow_cursor = true;
        }
    }

    fn undo(&mut self, undo: bool) {
        let result = if undo {
            self.doc_mut().undo()
        } else {
            self.doc_mut().redo()
        };
        self.prefixes.remove(&self.session.current().id);
        self.end_cuts.remove(&self.session.current().id);
        self.focus = Focus::Data;
        self.follow_cursor = true;
        self.status = match result {
            Ok(()) => {
                if undo {
                    "Undone".to_string()
                } else {
                    "Redone".to_string()
                }
            }
            Err(err) => err.message,
        };
        self.commit_input();
    }

    /// Remember the document as it is now. Escape will not undo past this.
    fn commit_input(&mut self) {
        let id = self.session.current().id;
        let depth = self.doc().undo_depth();
        self.committed_undo.insert(id, depth);
    }

    /// Throw away the command line, prefix text, and data typed since the last Enter.
    fn discard_pending(&mut self) {
        let id = self.session.current().id;
        let had_command = !self.command.is_empty();
        let had_prefix = self
            .prefixes
            .get(&id)
            .is_some_and(|map| map.values().any(|text| !text.is_empty()));
        let had_end = self.end_cuts.get(&id).is_some_and(|map| !map.is_empty());
        if had_command {
            self.history_at = None;
        }
        self.command.clear();
        self.command_col = 1;
        self.command_scroll = 0;
        self.prefixes.remove(&id);
        self.end_cuts.remove(&id);
        self.mark = Mark::None;
        let depth = self.committed_undo.get(&id).copied().unwrap_or(0);
        let reverted = self.doc_mut().revert_to_depth(depth);
        if had_command || had_prefix || had_end || reverted {
            self.status = "Discarded".to_string();
            if reverted {
                self.follow_cursor = true;
            }
        }
    }

    fn cancel_blanked_copies(&mut self, id: u32) {
        use crate::document::PendingBlock;
        let Some(map) = self.prefixes.get(&id) else {
            return;
        };
        let blanked: Vec<(usize, String)> = map
            .iter()
            .filter(|(_, text)| text.trim().is_empty())
            .map(|(line, text)| (*line, text.clone()))
            .collect();
        let pending = self.doc().pending();
        let copy = matches!(
            pending,
            PendingBlock::CopyLine { .. }
                | PendingBlock::CopyOpen { .. }
                | PendingBlock::CopySpan { .. }
        );
        if copy && blanked.iter().any(|(line, _)| pending.covers(*line)) {
            self.doc_mut().set_pending(PendingBlock::None);
        }
        let mut cleared = false;
        for (line, text) in &blanked {
            let label_len = self
                .doc()
                .line(*line)
                .ok()
                .and_then(|row| row.label.as_ref().map(|name| name.chars().count()))
                .unwrap_or(0);
            if label_len > 0 && !pending.covers(*line) && text.chars().count() >= 1 {
                if self.doc_mut().set_label(*line, None).is_ok() {
                    cleared = true;
                }
            }
        }
        if cleared {
            self.status = "Bookmark cleared".to_string();
        }
        if let Some(map) = self.prefixes.get_mut(&id) {
            for (line, _) in blanked {
                map.remove(&line);
            }
        }
    }

    /// CUT on the same Enter as `C` or `CC` copies that range instead of waiting.
    fn cut_owns_pending_copy(&self, command: &str) -> bool {
        use crate::document::PendingBlock;
        is_cut_command(command)
            && matches!(
                self.doc().pending(),
                PendingBlock::CopyLine { .. }
                    | PendingBlock::CopySpan { .. }
                    | PendingBlock::CopyOpen { .. }
            )
    }

    fn copy_destination_conflict(&self, pending: &[(usize, String)]) -> bool {
        use crate::document::PendingBlock;
        if matches!(
            self.doc().pending(),
            PendingBlock::After { .. } | PendingBlock::Before { .. }
        ) {
            return true;
        }
        pending.iter().any(|(_, text)| is_ab_prefix(text))
    }

    /// Enter: pair copy/move with A/B, run other prefix commands, then the command line.
    fn attention(&mut self) {
        let from_command = self.focus == Focus::Command;
        if from_command {
            let screen = self.screen_col();
            if self.command.trim().is_empty() {
                self.land_on_shown_line();
            }
            self.place_nearest_editor(screen);
        }
        self.mark = Mark::None;
        self.apply_end_cuts();
        let dropped_blank = self.doc_mut().cleanup_unused_inserts();
        let id = self.session.current().id;
        self.cancel_blanked_copies(id);
        let pending: Vec<(usize, String)> = self
            .prefixes
            .get(&id)
            .map(|map| {
                map.iter()
                    .filter(|(_, text)| !text.trim().is_empty())
                    .map(|(line, text)| (*line, text.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let command_text = self.command.trim().to_string();
        if is_cut_command(&command_text) && self.copy_destination_conflict(&pending) {
            self.reject_primary("command conflict");
            return;
        }

        let mut hold_line = dropped_blank;
        let mut data_cursor = false;
        if !pending.is_empty() {
            match apply_prefixes(&mut self.session.current_mut().doc, &pending) {
                Ok(outcome) => {
                    if let Some(map) = self.prefixes.get_mut(&id) {
                        for line in outcome.done {
                            map.remove(&line);
                        }
                    }
                    if !outcome.message.is_empty() {
                        self.status = outcome.message;
                    }
                    if outcome.stop && !self.cut_owns_pending_copy(&command_text) {
                        self.land_on_number_column();
                        return;
                    }
                    hold_line |= outcome.hold_line;
                    data_cursor = outcome.data_cursor;
                    self.restore_line_numbers();
                }
                Err(err) => {
                    self.status = err.message;
                    self.land_on_number_column();
                    return;
                }
            }
        }

        let command = command_text;
        if !command.is_empty() {
            if self.history.last().map(String::as_str) != Some(command.as_str()) {
                self.history.push(command.clone());
            }
            self.history_at = None;
        }
        if command.is_empty() {
            if pending.is_empty() {
                match reopen_insert(&mut self.session.current_mut().doc) {
                    Ok(true) => {
                        hold_line = true;
                        data_cursor = true;
                        self.status = "Inserted 1 line".to_string();
                    }
                    Ok(false) => {}
                    Err(err) => {
                        self.status = err.message;
                        self.land_on_number_column();
                        return;
                    }
                }
            }
            self.restore_line_numbers();
            if !from_command && self.focus != Focus::Command && !hold_line {
                self.down();
            }
            if data_cursor {
                self.focus = Focus::Data;
                self.follow_cursor = true;
                return;
            }
            if from_command {
                self.follow_cursor = true;
                return;
            }
            self.land_on_number_column();
            return;
        }
        self.arm_screen_find();
        let ran = execute(&mut self.session, &command);
        self.doc_mut().set_screen_find_line(None);
        match ran {
            Ok(message) => {
                self.command.clear();
                self.command_col = 1;
                self.restore_line_numbers();
                if command_locates(&command) {
                    self.scroll_second = true;
                    self.focus = Focus::Data;
                    self.parked_focus = Focus::Data;
                    self.follow_cursor = true;
                } else if self.doc().cursor_on_find() {
                    self.land_on_number_column();
                } else {
                    self.remember_editor();
                    self.focus = Focus::Command;
                    self.follow_cursor = true;
                }
                self.status = if message.is_empty() {
                    "Ok".to_string()
                } else {
                    message
                };
            }
            Err(err) => self.reject_primary(err.message),
        }
    }

    /// A rejected primary command stays on the line. The caret moves to its first character.
    fn reject_primary(&mut self, message: impl Into<String>) {
        self.status = message.into();
        let col = self
            .command
            .chars()
            .position(|ch| !ch.is_whitespace())
            .map(|index| index + 1)
            .unwrap_or(1);
        self.remember_editor();
        self.focus = Focus::Command;
        self.place_command_col(col);
    }

    /// Enter ends in column 1 of the line-number field.
    /// A FIND leaves the cursor on the first character of the match instead.
    fn land_on_number_column(&mut self) {
        self.follow_cursor = true;
        if self.doc().cursor_on_find() {
            self.focus = Focus::Data;
            return;
        }
        self.focus = Focus::Prefix;
        self.prefix_col = 1;
    }

    /// After Enter succeeds, the number column shows the current sequence
    /// numbers again. A command that is still pending keeps its mark.
    fn restore_line_numbers(&mut self) {
        self.doc_mut().renumber();
        let id = self.session.current().id;
        self.prefixes.remove(&id);
    }

    fn doc(&self) -> &Document {
        &self.session.current().doc
    }

    fn doc_mut(&mut self) -> &mut Document {
        &mut self.session.current_mut().doc
    }

    /// The editor rows currently on screen. Down from the command line uses this.
    pub fn note_screen(&mut self, top: usize, page: usize) {
        self.screen_top = top;
        self.screen_page = page;
    }

    /// FIND and CHANGE NEXT begin on the first line the screen is showing.
    fn arm_screen_find(&mut self) {
        let line = if self.screen_page == 0 {
            None
        } else {
            let rows = self.doc().visible_rows();
            if rows.is_empty() {
                None
            } else {
                let top = self.screen_top.min(rows.len() - 1);
                Some(rows[top].line)
            }
        };
        self.doc_mut().set_screen_find_line(line);
    }

    /// Leave the command line on the first editor row the screen is showing.
    fn land_on_shown_line(&mut self) {
        if self.screen_page == 0 {
            return;
        }
        let rows = self.doc().visible_rows();
        if rows.is_empty() {
            return;
        }
        let top = self.screen_top.min(rows.len() - 1);
        let col = self.cursor().1;
        let _ = self.doc_mut().set_cursor(rows[top].line, col);
    }

    /// Keep the cursor on the same screen row when the viewport moves.
    /// A cursor that was off the screen lands on the nearest visible row.
    pub fn slide_with_scroll(&mut self, old_top: usize, new_top: usize, page: usize) {
        if new_top == old_top {
            return;
        }
        let rows = self.doc().visible_rows();
        if rows.is_empty() {
            return;
        }
        let page = page.max(1);
        let (line, col) = self.cursor();
        let idx = rows.iter().position(|row| row.line == line).unwrap_or(0);
        let offset = idx as isize - old_top as isize;
        let offset = if offset < 0 {
            0
        } else if offset >= page as isize {
            page as isize - 1
        } else {
            offset
        };
        let last = rows.len() - 1;
        let new_idx = (new_top as isize + offset).clamp(0, last as isize) as usize;
        let _ = self.doc_mut().set_cursor(rows[new_idx].line, col);
    }

    /// After F7 or F8, stay on the original line when that line is still in
    /// the window. If the scroll carried it off the page, move to the nearest
    /// line that is still shown.
    pub fn keep_line_if_visible(&mut self, new_top: usize, page: usize) {
        let rows = self.doc().visible_rows();
        if rows.is_empty() {
            return;
        }
        let page = page.max(1);
        let (line, col) = self.cursor();
        let Some(idx) = rows.iter().position(|row| row.line == line) else {
            return;
        };
        let end = (new_top + page).min(rows.len());
        let new_top = new_top.min(rows.len().saturating_sub(1));
        if idx >= new_top && idx < end {
            return;
        }
        let new_idx = if idx < new_top {
            new_top
        } else {
            end.saturating_sub(1)
        };
        let _ = self.doc_mut().set_cursor(rows[new_idx].line, col);
    }

    /// CSR scroll. In the data, the cursor line becomes the new top (F8) or
    /// the new bottom (F7), and the cursor column becomes the left edge (F11)
    /// or the right edge (F10). A second press, when the cursor is already on
    /// that edge, moves by a screen.
    pub fn scroll_by_cursor(
        &mut self,
        dir: ScrollDir,
        screen_rows: usize,
        screen_cols: usize,
        top: usize,
        left: usize,
    ) -> (usize, usize) {
        let screen_rows = screen_rows.max(1);
        let screen_cols = screen_cols.max(1);
        let rows = self.doc().visible_rows();
        let max_top = rows.len().saturating_sub(screen_rows);
        let top = top.min(max_top);
        let page = screen_rows.saturating_sub(1).max(1);
        let (cursor_line, cursor_col) = self.cursor();
        let cursor_at = rows
            .iter()
            .position(|row| row.line == cursor_line)
            .unwrap_or(0);
        let col0 = cursor_col.max(1) - 1;
        let from_left = col0.saturating_sub(left);

        if self.focus == Focus::Command {
            return match dir {
                ScrollDir::Up => (top.saturating_sub(page), left),
                ScrollDir::Down => (downward_page(top, screen_rows, rows.len()), left),
                ScrollDir::Left => (top, left.saturating_sub(page_cols(screen_cols))),
                ScrollDir::Right => (top, left.saturating_add(page_cols(screen_cols))),
            };
        }
        if self.focus == Focus::Prefix && matches!(dir, ScrollDir::Left | ScrollDir::Right) {
            let amount = page_cols(screen_cols);
            return match dir {
                ScrollDir::Left => (top, left.saturating_sub(amount)),
                _ => (top, left.saturating_add(amount)),
            };
        }

        match dir {
            ScrollDir::Down => {
                let new_top = if cursor_at > max_top {
                    // A line on the last page becomes the top, even when the
                    // screen below it is not full.
                    cursor_at
                } else if cursor_at == top {
                    downward_page(top, screen_rows, rows.len())
                } else {
                    cursor_at.min(max_top)
                };
                (new_top, left)
            }
            ScrollDir::Up => {
                let force = cursor_at.saturating_sub(screen_rows.saturating_sub(1));
                let new_top = if force != top {
                    force
                } else {
                    top.saturating_sub(page)
                };
                (new_top.min(max_top), left)
            }
            ScrollDir::Right => {
                if from_left == 0 {
                    let amount = page_cols(screen_cols);
                    let _ = self.doc_mut().set_cursor(cursor_line, cursor_col + amount);
                    (top, left.saturating_add(amount))
                } else {
                    (top, col0)
                }
            }
            ScrollDir::Left => {
                let on_screen = col0 >= left && col0 < left + screen_cols;
                // The caret column becomes the rightmost column on the screen.
                let at_right = on_screen && col0 + 1 == left + screen_cols;
                if !on_screen || at_right {
                    (top, left.saturating_sub(page_cols(screen_cols)))
                } else {
                    (top, col0.saturating_sub(screen_cols.saturating_sub(1)))
                }
            }
        }
    }
}

/// F8 by a page. From the page before the tail, the first line not already
/// on the screen becomes the top, so only the remaining lines are shown.
fn is_cut_command(command: &str) -> bool {
    let (verb, _) = command
        .split_once(char::is_whitespace)
        .unwrap_or((command, ""));
    verb.eq_ignore_ascii_case("CUT")
}

/// The last statement of a successful command line is LOCATE, including `.label`.
fn command_locates(command: &str) -> bool {
    command
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .next_back()
        .is_some_and(|statement| {
            if statement.starts_with('.') {
                return true;
            }
            let verb = statement.split_whitespace().next().unwrap_or("");
            matches!(
                verb.to_ascii_uppercase().as_str(),
                "L" | "LOC" | "LOCATE" | "RL" | "RLOC" | "RLOCATE"
            )
        })
}

fn is_ab_prefix(text: &str) -> bool {
    let letters: String = text
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let letters = letters.to_ascii_uppercase();
    letters == "A" || letters == "B"
}

fn downward_page(top: usize, screen_rows: usize, row_count: usize) -> usize {
    let screen_rows = screen_rows.max(1);
    let max_top = row_count.saturating_sub(screen_rows);
    let step = screen_rows.saturating_sub(1).max(1);
    let next = top.saturating_add(step);
    let first_unseen = top.saturating_add(screen_rows);
    let last = row_count.saturating_sub(1);
    if next > max_top && first_unseen <= last {
        first_unseen
    } else {
        next.min(max_top)
    }
}

fn screen_number(word: &str) -> Option<usize> {
    if word.is_empty() || !word.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    word.parse::<usize>().ok().filter(|n| *n > 0)
}

fn is_space(ch: char) -> bool {
    ch.is_whitespace()
}

/// `col` is the 1-based caret. The result stays on this line.
fn next_nonspace_col(chars: &[char], col: usize) -> usize {
    let len = chars.len();
    if col == 0 || col > len {
        return len + 1;
    }
    let idx = col - 1;
    if is_space(chars[idx]) {
        for i in idx + 1..len {
            if !is_space(chars[i]) {
                return i + 1;
            }
        }
        return len + 1;
    }
    let mut i = idx + 1;
    while i < len && !is_space(chars[i]) {
        i += 1;
    }
    while i < len && is_space(chars[i]) {
        i += 1;
    }
    if i >= len {
        len + 1
    } else {
        i + 1
    }
}

fn prev_nonspace_col(chars: &[char], col: usize) -> usize {
    let len = chars.len();
    if col <= 1 || chars.is_empty() {
        return 1;
    }
    if col > len {
        return word_start(chars, len - 1);
    }
    let current = col - 1;
    if is_space(chars[current]) {
        let Some(word) = previous_word(chars, current) else {
            return 1;
        };
        return word_start(chars, word);
    }
    if current > 0 && is_space(chars[current - 1]) {
        let Some(word) = previous_word(chars, current - 1) else {
            return 1;
        };
        return word_start(chars, word);
    }
    word_start(chars, current)
}

fn previous_word(chars: &[char], from: usize) -> Option<usize> {
    let mut i = from;
    while i > 0 && is_space(chars[i]) {
        i -= 1;
    }
    if is_space(chars[i]) {
        return None;
    }
    Some(i)
}

fn word_start(chars: &[char], index: usize) -> usize {
    if chars.is_empty() {
        return 1;
    }
    let mut i = index.min(chars.len() - 1);
    while i > 0 && !is_space(chars[i]) && !is_space(chars[i - 1]) {
        i -= 1;
    }
    if is_space(chars[i]) {
        1
    } else {
        i + 1
    }
}

fn order_points(
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

fn blank_range(
    doc: &mut Document,
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
) -> EditResult<()> {
    if (start_line, start_col) == (end_line, end_col) {
        return Ok(());
    }
    let mut insert = String::new();
    for line in start_line..=end_line {
        if line > start_line {
            insert.push('\n');
        }
        let len = doc.line(line).map(|row| row.char_len()).unwrap_or(0);
        let from = if line == start_line { start_col } else { 1 };
        let to = if line == end_line {
            end_col
        } else {
            len.saturating_add(1).max(from)
        };
        insert.push_str(&" ".repeat(to.saturating_sub(from)));
    }
    doc.splice_text(start_line, start_col, end_line, end_col, &insert)
}

fn text_between(
    doc: &Document,
    a_line: usize,
    a_col: usize,
    b_line: usize,
    b_col: usize,
) -> String {
    let ((start_line, start_col), (end_line, end_col)) = order_points(a_line, a_col, b_line, b_col);
    if (start_line, start_col) == (end_line, end_col) {
        return String::new();
    }
    let slice = |line: usize, from: usize, to: usize| -> String {
        let chars = doc.line(line).map(|row| row.chars()).unwrap_or_default();
        let from = from.saturating_sub(1).min(chars.len());
        let to = to.saturating_sub(1).min(chars.len());
        chars[from..to].iter().collect()
    };
    if start_line == end_line {
        return slice(start_line, start_col, end_col);
    }
    let mut parts = Vec::new();
    parts.push(slice(start_line, start_col, usize::MAX));
    for line in start_line + 1..end_line {
        parts.push(slice(line, 1, usize::MAX));
    }
    parts.push(slice(end_line, 1, end_col));
    parts.join("\n")
}

fn page_cols(screen_cols: usize) -> usize {
    screen_cols.saturating_sub(1).max(1)
}

fn prepend_gap(text: &str, gap: usize) -> String {
    let spaces = " ".repeat(gap);
    match text.split_once('\n') {
        Some((first, rest)) => format!("{spaces}{first}\n{rest}"),
        None => format!("{spaces}{text}"),
    }
}

fn pad8(text: &str) -> String {
    let mut out: String = text.chars().take(8).collect();
    while out.chars().count() < 8 {
        out.push(' ');
    }
    out
}

fn insert_at(text: &mut String, col: &mut usize, ch: char) {
    let mut chars: Vec<char> = text.chars().collect();
    let at = (*col - 1).min(chars.len());
    chars.insert(at, ch);
    *text = chars.into_iter().collect();
    *col = at + 2;
}

fn replace_at(text: &mut String, col: usize, ch: char) {
    let mut chars: Vec<char> = text.chars().collect();
    let at = col.saturating_sub(1);
    if at < chars.len() {
        chars[at] = ch;
        *text = chars.into_iter().collect();
    }
}

fn delete_before(text: &mut String, col: &mut usize) {
    if *col <= 1 {
        return;
    }
    let mut chars: Vec<char> = text.chars().collect();
    let at = (*col - 2).min(chars.len().saturating_sub(1));
    if at < chars.len() {
        chars.remove(at);
    }
    *text = chars.into_iter().collect();
    *col = at + 1;
}

fn delete_at(text: &mut String, col: usize) {
    let mut chars: Vec<char> = text.chars().collect();
    let at = col - 1;
    if at < chars.len() {
        chars.remove(at);
        *text = chars.into_iter().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_inserts_on_the_current_line() {
        let mut view = EditorView::with_text("ab\n");
        view.handle_key(EditorKey::Char('X'));
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "Xab");
        assert_eq!(view.cursor(), (2, 2));
        view.handle_key(EditorKey::Backspace);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "ab");
    }

    #[test]
    fn escape_discards_text_typed_since_enter() {
        let mut view = EditorView::with_text("ab\n");
        view.click(PointerTarget::Data { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('X'));
        view.handle_key(EditorKey::Char('Y'));
        view.handle_key(EditorKey::Escape);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "ab");
        assert!(!view.session.current().doc.is_modified());
        assert_eq!(view.status(), "Discarded");

        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('D'));
        assert!(view.prefix_pending(2));
        view.handle_key(EditorKey::Escape);
        assert!(!view.prefix_pending(2));

        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('F'));
        view.handle_key(EditorKey::Escape);
        assert!(view.command().is_empty());
        assert_eq!(view.command_col(), 1);

        view.click(PointerTarget::Data { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('Q'));
        view.click(PointerTarget::Command { col: 1 });
        for ch in "COLS".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "Qab");
        assert!(view.session.current().doc.cols_mode());
        view.click(PointerTarget::Data { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('Z'));
        view.handle_key(EditorKey::Escape);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "Qab");
        assert!(view.session.current().doc.cols_mode());
    }

    #[test]
    fn command_caret_moves_across_blank_columns_while_cols_is_on() {
        let mut view = EditorView::with_text("ab\n");
        view.click(PointerTarget::Command { col: 1 });
        for ch in "COLS".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert!(view.session.current().doc.cols_mode());
        assert_eq!(view.focus(), Focus::Command);
        assert!(view.command().is_empty());
        view.set_command_field_width(40);
        view.handle_key(EditorKey::Right);
        view.handle_key(EditorKey::Right);
        assert_eq!(view.command_col(), 3);
        view.click(PointerTarget::Command { col: 12 });
        assert_eq!(view.command_col(), 12);
        view.handle_key(EditorKey::Backspace);
        assert_eq!(view.command_col(), 11);
        assert!(view.command().is_empty());
        view.handle_key(EditorKey::Char('Z'));
        assert_eq!(view.command(), "          Z");
        assert_eq!(view.command_col(), 12);
    }

    #[test]
    fn reopen_discards_unsaved_edits_in_the_same_tab() {
        let path = std::env::temp_dir().join(format!("ispflite-reopen-{}.txt", std::process::id()));
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let mut view = EditorView::new(Session::new());
        view.open_path(&path);
        assert_eq!(view.session.tabs().len(), 1);
        let id = view.session.current().id;
        view.handle_key(EditorKey::Char('X'));
        assert!(view.session.current().doc.is_modified());
        assert!(view.reopen_current());
        assert_eq!(view.session.tabs().len(), 1);
        assert_eq!(view.session.current().id, id);
        assert_eq!(view.session.current().path.as_deref(), Some(path.as_path()));
        assert!(!view.session.current().doc.is_modified());
        assert_eq!(view.session.current().doc.to_text(), "one\ntwo\n");
        assert!(view.status().starts_with("Re-opened "));

        let _ = std::fs::remove_file(&path);
        view.handle_key(EditorKey::Char('Q'));
        assert!(!view.reopen_current());
        assert!(view.session.current().doc.is_modified());
        assert!(view.status().contains("Cannot open"));
    }

    #[test]
    fn overtype_replaces_the_character() {
        let mut view = EditorView::with_text("ab\n");
        view.handle_key(EditorKey::ToggleInsert);
        view.handle_key(EditorKey::Char('Z'));
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "Zb");

        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Char('C'));
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('D'));
        assert_eq!(view.shown_prefix(2, "00000001"), "DC      ");
        view.handle_key(EditorKey::ToggleInsert);
        view.click(PointerTarget::Prefix { line: 2, col: 2 });
        view.handle_key(EditorKey::Char('X'));
        assert_eq!(view.shown_prefix(2, "00000001"), "DXC     ");

        view.click(PointerTarget::Command { col: 1 });
        for ch in "FIND".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::ToggleInsert);
        view.handle_key(EditorKey::Char('X'));
        assert_eq!(view.command(), "XIND");
        view.handle_key(EditorKey::ToggleInsert);
        view.click(PointerTarget::Command { col: 2 });
        view.handle_key(EditorKey::Char('Y'));
        assert_eq!(view.command(), "XYIND");
    }

    #[test]
    fn prefix_delete_runs_on_enter() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('D'));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.to_text(), "one\n");
        assert!(!view.prefix_pending(3));
    }

    #[test]
    fn find_and_change_start_on_the_first_shown_line() {
        let mut view = EditorView::with_text("alpha\nbeta\nalpha\n");
        view.click(PointerTarget::Data { line: 4, col: 1 });
        view.note_screen(1, 2);
        view.click(PointerTarget::Command { col: 1 });
        for ch in "FIND alpha".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert!(view.status().contains("line 1"), "{}", view.status());
        assert_eq!(view.cursor(), (2, 1));

        view.click(PointerTarget::Data { line: 4, col: 1 });
        view.note_screen(1, 2);
        view.click(PointerTarget::Command { col: 1 });
        for ch in "C alpha ALPHA".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "ALPHA");
        assert_eq!(view.session.current().doc.line_text(4).unwrap(), "alpha");
    }

    #[test]
    fn command_line_finds_and_moves_the_cursor() {
        let mut view = EditorView::with_text("alpha\nbeta\n");
        view.handle_key(EditorKey::Tab);
        assert_eq!(view.focus(), Focus::Command);
        for ch in "FIND beta".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert!(view.status().contains("Found"), "{}", view.status());
        assert_eq!(view.cursor(), (3, 1));
        assert_eq!(view.focus(), Focus::Data);
        assert_eq!(view.session.current().doc.find_spans(3), vec![(1, 4)]);
        assert!(view.command().is_empty());
    }

    #[test]
    fn locate_asks_to_show_the_target_as_the_second_line() {
        let mut view = EditorView::with_text("one\ntwo\nthree\nfour\n");
        view.handle_key(EditorKey::Tab);
        for ch in "L 3".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert!(
            view.status().contains("Cursor on line 3"),
            "{}",
            view.status()
        );
        assert_eq!(view.cursor(), (4, 1));
        assert_eq!(view.focus(), Focus::Data);
        assert!(view.take_scroll_second());
        assert!(!view.take_scroll_second());

        view.click(PointerTarget::Command { col: 1 });
        for ch in "L 99".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert!(!view.take_scroll_second());
    }

    #[test]
    fn prefix_copy_and_after_run_together() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('A'));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.to_text(), "one\ntwo\none\n");
        assert!(!view.prefix_pending(2));
        assert_eq!(view.status(), "Copied");
    }

    #[test]
    fn cut_with_c_copies_the_marked_line_and_clears_it() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.click(PointerTarget::Command { col: 1 });
        for ch in "CUT".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.clip(), ["one"]);
        assert!(!view.prefix_pending(2));
        assert_eq!(view.focus(), Focus::Command);
        assert!(view.command().is_empty());
    }

    #[test]
    fn blanking_a_pending_c_cancels_it() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Enter);
        assert!(view.prefix_pending(2));
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char(' '));
        view.handle_key(EditorKey::Enter);
        assert!(!view.prefix_pending(2));
        assert_eq!(
            view.session.current().doc.pending(),
            crate::document::PendingBlock::None
        );
    }

    #[test]
    fn a_lone_pending_command_can_be_deleted_or_overtyped() {
        use crate::document::PendingBlock;
        let mut view = EditorView::with_text("one\ntwo\nthree\n");
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('D'));
        view.handle_key(EditorKey::Char('D'));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.shown_prefix(2, "00000001"), "DD      ");
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Delete);
        view.handle_key(EditorKey::Delete);
        assert!(!view.prefix_pending(2));
        assert_eq!(view.session.current().doc.pending(), PendingBlock::None);

        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Enter);
        view.handle_key(EditorKey::ToggleInsert);
        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('X'));
        assert_eq!(view.shown_prefix(3, "00000002"), "X       ");
        assert_eq!(view.session.current().doc.pending(), PendingBlock::None);

        view.handle_key(EditorKey::ToggleInsert);
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Enter);
        view.click(PointerTarget::Prefix { line: 4, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Enter);
        assert_ne!(view.session.current().doc.pending(), PendingBlock::None);
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Delete);
        assert_ne!(view.session.current().doc.pending(), PendingBlock::None);
        assert_eq!(view.shown_prefix(2, "00000001"), "CC      ");
    }

    #[test]
    fn cut_rejects_an_a_destination() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('A'));
        view.click(PointerTarget::Command { col: 1 });
        for ch in "CUT".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.status(), "command conflict");
        assert_eq!(view.focus(), Focus::Command);
        assert_eq!(view.command_col(), 1);
        assert_eq!(view.command(), "CUT");
        assert_eq!(view.session.current().doc.to_text(), "one\ntwo\n");
        assert!(view.session.clip().is_empty());
    }

    #[test]
    fn a_rejected_command_puts_the_caret_on_its_first_character() {
        let mut view = EditorView::with_text("alpha\n");
        view.click(PointerTarget::Command { col: 1 });
        for ch in "  FIND missing".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert!(view.status().contains("not found"), "{}", view.status());
        assert_eq!(view.focus(), Focus::Command);
        assert_eq!(view.command(), "  FIND missing");
        assert_eq!(view.command_col(), 3);
    }

    #[test]
    fn a_copy_without_a_destination_stays_on_the_prefix() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('C'));
        view.handle_key(EditorKey::Enter);
        assert!(view.status().contains("Pending"), "{}", view.status());
        assert!(view.shown_prefix(2, "00000001").starts_with('C'));
        assert!(!view.prefix_pending(3));
        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('A'));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.to_text(), "one\ntwo\none\n");
    }

    #[test]
    fn down_from_the_command_line_lands_on_the_shown_line() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\ng\nh\n");
        view.click(PointerTarget::Data { line: 8, col: 1 });
        view.click(PointerTarget::Command { col: 1 });
        view.note_screen(4, 3);
        let shown = view.session.current().doc.visible_rows()[4].line;
        assert_ne!(view.cursor().0, shown);
        view.handle_key(EditorKey::Down);
        assert_eq!(view.focus(), Focus::Data);
        assert_eq!(view.cursor(), (shown, 1));

        view.click(PointerTarget::Data { line: 8, col: 1 });
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Tab);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.cursor().0, shown);

        view.click(PointerTarget::Data { line: 8, col: 2 });
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Enter);
        assert_ne!(view.focus(), Focus::Command);
        assert_eq!(view.cursor().0, shown);
    }

    #[test]
    fn up_from_the_top_line_reaches_the_command_line() {
        let mut view = EditorView::with_text("only\n");
        view.handle_key(EditorKey::Up);
        view.handle_key(EditorKey::Up);
        assert_eq!(view.focus(), Focus::Command);
        view.handle_key(EditorKey::Down);
        assert_eq!(view.focus(), Focus::Data);
    }

    #[test]
    fn arrow_between_command_and_editor_keeps_the_nearest_column() {
        let mut view = EditorView::with_text("abcdefghijklmnopqrstuvwxyz\n");
        view.click(PointerTarget::Data { line: 2, col: 10 });
        view.handle_key(EditorKey::Up);
        view.handle_key(EditorKey::Up);
        assert_eq!(view.focus(), Focus::Command);
        assert_eq!(view.command_col(), 10);
        view.handle_key(EditorKey::Down);
        assert_eq!(view.focus(), Focus::Data);
        assert_eq!(view.cursor(), (1, 10));

        view.click(PointerTarget::Prefix { line: 1, col: 4 });
        view.handle_key(EditorKey::Up);
        assert_eq!(view.focus(), Focus::Command);
        assert_eq!(view.command_col(), 1);
        view.handle_key(EditorKey::Down);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.prefix_col(), 4);
        assert_eq!(view.cursor().1, 10);

        view.set_cross_layout(20, 0, 80);
        view.click(PointerTarget::Data { line: 2, col: 1 });
        view.handle_key(EditorKey::Up);
        view.handle_key(EditorKey::Up);
        assert_eq!(view.command_col(), 21);
        view.handle_key(EditorKey::Down);
        assert_eq!(view.focus(), Focus::Data);
        assert_eq!(view.cursor().1, 1);

        view.click(PointerTarget::Prefix { line: 1, col: 4 });
        view.handle_key(EditorKey::Up);
        assert_eq!(view.command_col(), 15);
        view.handle_key(EditorKey::Down);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.prefix_col(), 4);
    }

    #[test]
    fn enter_from_the_command_line_moves_the_caret() {
        let mut view = EditorView::with_text("abcdefghijklmnopqrstuvwxyz\n");
        view.click(PointerTarget::Data { line: 2, col: 4 });
        view.handle_key(EditorKey::Up);
        view.handle_key(EditorKey::Up);
        view.handle_key(EditorKey::Right);
        view.handle_key(EditorKey::Right);
        assert_eq!(view.command_col(), 6);
        assert_eq!(view.cursor().1, 4);
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.cursor().1, 6);
        assert_ne!(view.focus(), Focus::Command);
    }

    #[test]
    fn overlay_space_clears_a_label() {
        let mut view = EditorView::with_text("abcdef\n");
        view.session
            .current_mut()
            .doc
            .set_label(2, Some(".HERE"))
            .unwrap();
        view.click(PointerTarget::Prefix { line: 2, col: 3 });
        view.handle_key(EditorKey::Char(' '));
        assert_eq!(
            view.session.current().doc.line(2).unwrap().label.as_deref(),
            Some(".HERE")
        );
        assert!(view.prefix_pending(2));
        view.handle_key(EditorKey::Enter);
        assert!(view.session.current().doc.line(2).unwrap().label.is_none());
        assert_eq!(view.status(), "Bookmark cleared");
        assert!(!view.prefix_pending(2));

        view.session
            .current_mut()
            .doc
            .set_label(2, Some(".HERE"))
            .unwrap();
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('D'));
        view.handle_key(EditorKey::Char(' '));
        assert_eq!(
            view.session.current().doc.line(2).unwrap().label.as_deref(),
            Some(".HERE")
        );
        assert!(view.prefix_pending(2));
    }

    #[test]
    fn left_arrow_enters_the_number_column() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.handle_key(EditorKey::Left);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.prefix_col(), 8);
        view.handle_key(EditorKey::Left);
        assert_eq!(view.prefix_col(), 7);
        view.handle_key(EditorKey::Up);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.cursor().0, 1);
        view.handle_key(EditorKey::Right);
        view.handle_key(EditorKey::Right);
        assert_eq!(view.focus(), Focus::Data);
        assert_eq!(view.cursor(), (1, 1));
    }

    #[test]
    fn successful_enter_shows_the_new_line_numbers() {
        let mut view = EditorView::with_text("one\ntwo\nthree\n");
        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('D'));
        assert!(view.shown_prefix(3, "00000002").starts_with('D'));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.to_text(), "one\nthree\n");
        let first = view.session.current().doc.line(2).unwrap().display_prefix();
        let second = view.session.current().doc.line(3).unwrap().display_prefix();
        assert_eq!(view.shown_prefix(2, &first), "00000001");
        assert_eq!(view.shown_prefix(3, &second), "00000002");
        assert_eq!(first, "00000001");
        assert_eq!(second, "00000002");
        assert_eq!(view.cursor().0, 3);
        assert_eq!(view.focus(), Focus::Prefix);
    }

    #[test]
    fn insert_stays_under_the_previous_word_until_enter_drops_it() {
        let mut view = EditorView::with_text("    alpha beta\n\nsecond\n");
        view.click(PointerTarget::Prefix { line: 3, col: 1 });
        view.handle_key(EditorKey::Char('I'));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.cursor(), (4, 5));
        assert_eq!(view.focus(), Focus::Data);
        assert!(view.session.current().doc.line(4).unwrap().is_insert());

        view.handle_key(EditorKey::Enter);
        assert_eq!(
            view.session.current().doc.to_text(),
            "    alpha beta\n\nsecond\n"
        );

        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('I'));
        view.handle_key(EditorKey::Enter);
        view.handle_key(EditorKey::Char('Z'));
        let (still_insert, prefix) = {
            let typed = view.session.current().doc.line(3).unwrap();
            (typed.is_insert(), typed.display_prefix())
        };
        assert!(still_insert);
        assert_eq!(prefix, "''''''''");
        view.handle_key(EditorKey::Enter);
        assert!(!view.session.current().doc.line(3).unwrap().is_insert());
        assert!(view
            .session
            .current()
            .doc
            .line_text(3)
            .unwrap()
            .contains('Z'));
        let opened = view.session.current().doc.line(4).unwrap();
        assert!(opened.is_insert());
        assert_eq!(opened.display_prefix(), "''''''''");
        assert_eq!(view.cursor(), (4, 5));
        assert_eq!(view.focus(), Focus::Data);
    }

    #[test]
    fn enter_lands_on_the_leftmost_number_column() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.prefix_col(), 1);
        assert_eq!(view.cursor().0, 3);
    }

    #[test]
    fn f12_retrieves_the_previous_command() {
        let mut view = EditorView::with_text("alpha beta\nbeta\n");
        view.click(PointerTarget::Command { col: 1 });
        for ch in "FIND beta".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.handle_key(EditorKey::Enter);
        assert!(view.command().is_empty());
        view.retrieve(false);
        assert_eq!(view.command(), "FIND beta");
        assert_eq!(view.focus(), Focus::Command);
        view.retrieve(false);
        assert_eq!(view.status(), "No further commands");
        view.retrieve(true);
        assert!(view.command().is_empty());
    }

    #[test]
    fn home_moves_to_the_command_line() {
        let mut view = EditorView::with_text("one\ntwo\n");
        view.click(PointerTarget::Data { line: 3, col: 2 });
        view.handle_key(EditorKey::Home);
        assert_eq!(view.focus(), Focus::Command);
        assert_eq!(view.command_col(), 1);
        assert_eq!(view.cursor(), (3, 2));
    }

    #[test]
    fn f8_keeps_the_cursor_on_a_line_that_stays_on_the_page() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\n");
        view.click(PointerTarget::Data { line: 4, col: 1 });
        let line = view.cursor().0;
        view.keep_line_if_visible(1, 4);
        assert_eq!(view.cursor().0, line);
    }

    #[test]
    fn f8_moves_the_cursor_when_its_line_leaves_the_page() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\n");
        let line = view.cursor().0;
        view.keep_line_if_visible(4, 4);
        assert_ne!(view.cursor().0, line);
        let rows = view.session.current().doc.visible_rows();
        let idx = rows
            .iter()
            .position(|row| row.line == view.cursor().0)
            .unwrap();
        assert_eq!(idx, 4);
    }

    #[test]
    fn page_scroll_moves_the_cursor_off_the_original_line() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\n");
        let before = view.cursor().0;
        view.slide_with_scroll(0, 3, 4);
        assert_ne!(view.cursor().0, before);
        let rows = view.session.current().doc.visible_rows();
        let idx = rows
            .iter()
            .position(|row| row.line == view.cursor().0)
            .unwrap();
        assert_eq!(idx, 4);
    }

    #[test]
    fn enter_moves_to_the_next_line_when_ctrl_is_attention() {
        let mut view = EditorView::with_text("a\nb\nc\n");
        view.click(PointerTarget::Data { line: 2, col: 1 });
        view.next_line(NextLineCursor::Text, 0);
        assert_eq!(view.cursor(), (3, 1));
        assert_eq!(view.focus(), Focus::Data);
        view.click(PointerTarget::Command { col: 1 });
        view.next_line(NextLineCursor::Text, 0);
        let first = view.session.current().doc.visible_rows()[0].line;
        assert_eq!(view.cursor().0, first);
        assert_eq!(view.focus(), Focus::Data);
        let last = view
            .session
            .current()
            .doc
            .visible_rows()
            .last()
            .unwrap()
            .line;
        view.click(PointerTarget::Data { line: last, col: 1 });
        view.next_line(NextLineCursor::Text, 0);
        assert_eq!(view.cursor().0, last);
    }

    #[test]
    fn next_line_from_the_command_line_uses_the_first_visible_line() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\n");
        view.click(PointerTarget::Data { line: 4, col: 1 });
        view.click(PointerTarget::Command { col: 1 });
        view.next_line(NextLineCursor::LineNumber, 2);
        let line = view.session.current().doc.visible_rows()[2].line;
        assert_eq!(view.cursor().0, line);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.prefix_col(), 1);
    }

    #[test]
    fn next_line_can_land_in_the_line_number() {
        let mut view = EditorView::with_text("a\nb\nc\n");
        view.click(PointerTarget::Data { line: 2, col: 1 });
        view.next_line(NextLineCursor::LineNumber, 0);
        assert_eq!(view.cursor().0, 3);
        assert_eq!(view.focus(), Focus::Prefix);
        assert_eq!(view.prefix_col(), 1);
    }

    #[test]
    fn typing_past_the_last_non_space_pads_the_line() {
        let mut view = EditorView::with_text("ab\n");
        view.click(PointerTarget::Data { line: 2, col: 1 });
        for _ in 0..5 {
            view.handle_key(EditorKey::Right);
        }
        assert_eq!(view.cursor(), (2, 6));
        view.handle_key(EditorKey::Char('Z'));
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "ab   Z");
        assert_eq!(view.cursor().1, 7);
    }

    #[test]
    fn alt_arrow_moves_to_the_next_non_space_on_the_line() {
        let mut view = EditorView::with_text("alpha beta  end\n");
        view.click(PointerTarget::Data { line: 2, col: 1 });
        view.jump_nonspace(true, false);
        assert_eq!(view.cursor(), (2, 7));
        view.jump_nonspace(true, false);
        assert_eq!(view.cursor(), (2, 13));
        view.jump_nonspace(true, false);
        assert_eq!(view.cursor(), (2, 16));
        view.jump_nonspace(false, false);
        assert_eq!(view.cursor(), (2, 13));
        view.click(PointerTarget::Data { line: 2, col: 3 });
        view.jump_nonspace(false, false);
        assert_eq!(view.cursor().1, 1);
        view.click(PointerTarget::Data { line: 2, col: 9 });
        view.jump_nonspace(false, false);
        assert_eq!(view.cursor().1, 7);
        view.jump_nonspace(false, false);
        assert_eq!(view.cursor().1, 1);
        view.click(PointerTarget::Data { line: 2, col: 6 });
        view.jump_nonspace(true, false);
        assert_eq!(view.cursor().1, 7);
    }

    #[test]
    fn shift_arrows_select_text_for_copy_cut_and_paste() {
        let mut view = EditorView::with_text("alpha beta\nsecond\n");
        view.click(PointerTarget::Data { line: 2, col: 1 });
        for _ in 0..5 {
            view.nudge(EditorKey::Right, true);
        }
        assert_eq!(view.data_selection(2), Some((1, 6)));
        assert_eq!(view.copy_selection().as_deref(), Some("alpha"));
        assert_eq!(view.cut_selection().as_deref(), Some("alpha"));
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), " beta");
        view.paste_text("ALPHA\nLINE");
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "ALPHA");
        assert_eq!(
            view.session.current().doc.line_text(3).unwrap(),
            "LINE beta"
        );

        let mut overlay = EditorView::with_text("alpha beta\n");
        overlay.handle_key(EditorKey::ToggleInsert);
        assert!(!overlay.insert_mode());
        overlay.click(PointerTarget::Data { line: 2, col: 1 });
        for _ in 0..5 {
            overlay.nudge(EditorKey::Right, true);
        }
        assert_eq!(overlay.cut_selection().as_deref(), Some("alpha"));
        assert_eq!(
            overlay.session.current().doc.line_text(2).unwrap(),
            "      beta"
        );
        overlay.click(PointerTarget::Data { line: 2, col: 1 });
        overlay.paste_text("XYZ");
        assert_eq!(
            overlay.session.current().doc.line_text(2).unwrap(),
            "XYZ   beta"
        );
        assert_eq!(overlay.cursor(), (2, 1));

        view.click(PointerTarget::Command { col: 1 });
        for ch in "find".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.nudge(EditorKey::Left, true);
        view.nudge(EditorKey::Left, true);
        assert_eq!(view.command_selection(), Some((3, 5)));
        assert_eq!(view.copy_selection().as_deref(), Some("nd"));
    }

    #[test]
    fn paste_pads_the_empty_columns_and_stays_on_the_first_character() {
        let mut view = EditorView::with_text("abc\n");
        view.click(PointerTarget::Data { line: 2, col: 6 });
        view.paste_text("XY");
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "abc  XY");
        assert_eq!(view.cursor(), (2, 6));

        view.click(PointerTarget::Data { line: 2, col: 2 });
        view.paste_text("Q\nR");
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "aQ");
        assert_eq!(view.session.current().doc.line_text(3).unwrap(), "Rbc  XY");
        assert_eq!(view.cursor(), (2, 2));

        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('a'));
        view.handle_key(EditorKey::Char('b'));
        view.click(PointerTarget::Command { col: 5 });
        view.paste_text("XY");
        assert_eq!(view.command(), "ab  XY");
        assert_eq!(view.command_col(), 5);
    }

    #[test]
    fn select_all_covers_every_data_line() {
        let mut view = EditorView::with_text("ab\ncd\n");
        view.select_all();
        assert_eq!(view.copy_selection().as_deref(), Some("ab\ncd"));

        let mut empty = EditorView::with_text("");
        empty.select_all();
        assert!(empty.copy_selection().is_none());
    }

    #[test]
    fn m_on_the_command_line_scrolls_to_the_edge() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\n");
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('M'));
        assert!(view.take_max_scroll());
        assert!(view.command().is_empty());
        view.scroll_max(false);
        let rows = view.session.current().doc.visible_rows();
        assert_eq!(view.cursor().0, rows.last().unwrap().line);
        assert_eq!(view.focus(), Focus::Command);
        assert!(view.take_follow());
        assert_eq!(view.status(), "Bottom of data");

        view.click(PointerTarget::Command { col: 1 });
        for ch in ['m', 'a', 'x'] {
            view.handle_key(EditorKey::Char(ch));
        }
        assert!(view.take_max_scroll());
        view.scroll_max(true);
        let rows = view.session.current().doc.visible_rows();
        assert_eq!(view.cursor().0, rows.first().unwrap().line);
        assert_eq!(view.status(), "Top of data");

        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('F'));
        assert!(!view.take_max_scroll());
        assert_eq!(view.command(), "F");

        let mut view = EditorView::with_text("abcdefghijklmnopqrstuvwxyz\nshort\n");
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('M'));
        assert!(view.take_max_scroll());
        assert_eq!(view.scroll_max_columns(true, 10), 16);
        assert_eq!(view.cursor().1, 26);
        assert_eq!(view.focus(), Focus::Command);
        assert_eq!(view.status(), "Right of data");

        view.click(PointerTarget::Command { col: 1 });
        for ch in ['m', 'a', 'x'] {
            view.handle_key(EditorKey::Char(ch));
        }
        assert!(view.take_max_scroll());
        assert_eq!(view.scroll_max_columns(false, 10), 0);
        assert_eq!(view.cursor().1, 1);
        assert_eq!(view.status(), "Left of data");

        let mut view = EditorView::with_text("abc\n");
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('M'));
        assert!(view.take_max_scroll());
        assert_eq!(view.scroll_max_columns(true, 10), 0);
        assert_eq!(view.cursor().1, 3);
        assert_eq!(view.status(), "Right of data");
    }

    #[test]
    fn a_number_on_the_command_line_scrolls_that_many_lines() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('3'));
        assert_eq!(view.take_line_scroll(), Some(3));
        assert!(view.command().is_empty());
        let top = view.scroll_by_lines(true, 3, 0, 4);
        assert_eq!(top, 3);
        assert_eq!(view.status(), "Down 3");
        let top = view.scroll_by_lines(true, 100, top, 4);
        assert_eq!(top, view.session.current().doc.visible_rows().len() - 4);
        assert_eq!(view.status(), "Bottom of data");
        let top = view.scroll_by_lines(false, 2, top, 4);
        assert_eq!(top, view.session.current().doc.visible_rows().len() - 6);
        assert_eq!(view.status(), "Up 2");
        assert_eq!(view.scroll_by_lines(false, 100, top, 4), 0);
        assert_eq!(view.status(), "Top of data");

        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('M'));
        assert_eq!(view.take_line_scroll(), None);
        assert_eq!(view.command(), "M");
        view.command = "0".to_string();
        assert_eq!(view.take_line_scroll(), None);
        assert_eq!(view.command(), "0");
        view.command = "05".to_string();
        assert_eq!(view.take_line_scroll(), Some(5));
        assert!(view.command().is_empty());

        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('4'));
        assert_eq!(view.take_line_scroll(), Some(4));
        assert_eq!(view.scroll_by_columns(true, 4, 0), 4);
        assert_eq!(view.status(), "Right 4");
        assert_eq!(view.scroll_by_columns(false, 4, 4), 0);
        assert_eq!(view.status(), "Left of data");
        assert_eq!(view.scroll_by_columns(false, 3, 10), 7);
        assert_eq!(view.status(), "Left 3");
    }

    #[test]
    fn f9_swaps_to_the_typed_screen_and_clears_the_number() {
        let mut view = EditorView::with_text("a\n");
        view.session.add_screen();
        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('1'));
        view.swap_key();
        assert!(view.command().is_empty());
        assert_eq!(view.command_col(), 1);
        assert_eq!(view.session.current_index(), 0);

        view.click(PointerTarget::Command { col: 1 });
        view.handle_key(EditorKey::Char('9'));
        view.swap_key();
        assert_eq!(view.command(), "9");
        assert_eq!(view.focus(), Focus::Command);
        assert_eq!(view.command_col(), 1);
        assert!(view.status().contains("not active"), "{}", view.status());
    }

    #[test]
    fn function_keys_scroll_by_the_cursor() {
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\n");
        // Visible rows: top, six data lines, bottom. Cursor starts on the first data line.
        let (top, left) = view.scroll_by_cursor(ScrollDir::Down, 4, 10, 0, 0);
        assert_eq!((top, left), (1, 0));
        let (top, _) = view.scroll_by_cursor(ScrollDir::Down, 4, 10, top, 0);
        assert_eq!(top, 4);

        let (top, _) = view.scroll_by_cursor(ScrollDir::Up, 4, 10, 4, 0);
        assert_eq!(top, 0);
        view.click(PointerTarget::Data { line: 6, col: 1 });
        let (top, _) = view.scroll_by_cursor(ScrollDir::Up, 4, 10, 0, 0);
        assert_eq!(top, 2);
        let (top, _) = view.scroll_by_cursor(ScrollDir::Down, 4, 10, 0, 0);
        assert_eq!(top, 5);

        let rows = view.session.current().doc.visible_rows();
        let last = rows.len() - 1;
        view.click(PointerTarget::Data {
            line: rows[last].line,
            col: 1,
        });
        let (top, _) = view.scroll_by_cursor(ScrollDir::Down, 4, 10, 0, 0);
        assert_eq!(top, last);

        let mut view = EditorView::with_text("abcdefghijklmnopqrstuvwxyz0123\n");
        view.click(PointerTarget::Data { line: 2, col: 1 });
        let (_, left) = view.scroll_by_cursor(ScrollDir::Right, 4, 10, 0, 0);
        assert_eq!(left, 9);
        assert_eq!(view.cursor().1, 10);
        view.click(PointerTarget::Data { line: 2, col: 15 });
        let (_, left) = view.scroll_by_cursor(ScrollDir::Right, 4, 10, 0, 0);
        assert_eq!(left, 14);
        view.click(PointerTarget::Data { line: 2, col: 20 });
        let (_, left) = view.scroll_by_cursor(ScrollDir::Left, 4, 10, 0, 12);
        assert_eq!(left, 10);
        view.land_on_rightmost_if_outside(left, 10);
        assert_eq!(view.cursor().1, 20);
        let (_, left) = view.scroll_by_cursor(ScrollDir::Left, 4, 10, 0, left);
        assert_eq!(left, 1);
        view.land_on_rightmost_if_outside(left, 10);
        assert_eq!(view.cursor().1, 11);
        view.click(PointerTarget::Data { line: 2, col: 20 });
        let left = view.scroll_by_columns(false, 5, 12);
        assert_eq!(left, 7);
        view.land_on_rightmost_if_outside(left, 10);
        assert_eq!(view.cursor().1, 17);

        // 12 visible rows, screen 4. Index 6 is the page before the tail
        // (rows 6–9 on screen, 10 and 11 still below). F8 shows those two.
        let mut view = EditorView::with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
        view.click(PointerTarget::Data { line: 7, col: 1 });
        let (top, _) = view.scroll_by_cursor(ScrollDir::Down, 4, 10, 6, 0);
        assert_eq!(top, 10);
        view.click(PointerTarget::Data { line: 4, col: 1 });
        let (top, _) = view.scroll_by_cursor(ScrollDir::Down, 4, 10, 3, 0);
        assert_eq!(top, 6);
    }

    #[test]
    fn end_hides_the_tail_until_enter_or_a_command() {
        let mut view = EditorView::with_text("abcdef\n");
        view.click(PointerTarget::Data { line: 2, col: 4 });
        view.handle_key(EditorKey::End);
        assert_eq!(view.shown_line(2, "abcdef").as_ref(), "abc");
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "abcdef");
        assert_eq!(view.status(), "Pending end of line");

        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "abc");
        assert!(view.pending_end(2).is_none());

        let mut view = EditorView::with_text("abcdef\n");
        view.click(PointerTarget::Data { line: 2, col: 4 });
        view.handle_key(EditorKey::End);
        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('.'));
        view.handle_key(EditorKey::Char('a'));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "abc");
        assert_eq!(
            view.session.current().doc.line(2).unwrap().label.as_deref(),
            Some(".A")
        );

        let mut view = EditorView::with_text("abcdef\n");
        view.click(PointerTarget::Data { line: 2, col: 4 });
        view.handle_key(EditorKey::End);
        let _ = view.run_primary("FIND xyz");
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "abc");

        let mut view = EditorView::with_text("abcdef\n");
        view.click(PointerTarget::Data { line: 2, col: 4 });
        view.handle_key(EditorKey::End);
        view.handle_key(EditorKey::Right);
        view.handle_key(EditorKey::Right);
        assert_eq!(view.cursor(), (2, 6));
        view.handle_key(EditorKey::End);
        assert_eq!(view.cursor(), (2, 6));
        assert_eq!(view.pending_end(2), Some(4));
        let hidden = view.session.current().doc.line_text(2).unwrap();
        assert_eq!(view.shown_line(2, &hidden).as_ref(), "abc");
        view.handle_key(EditorKey::Char('X'));
        let text = view.session.current().doc.line_text(2).unwrap();
        assert_eq!(view.shown_line(2, &text).as_ref(), "abc  X");
        assert!(text.ends_with("def"), "{text}");
        assert_eq!(view.cursor(), (2, 7));
        view.handle_key(EditorKey::Enter);
        assert_eq!(view.session.current().doc.line_text(2).unwrap(), "abc  X");
    }

    #[test]
    fn end_clears_a_typed_prefix_and_the_command() {
        let mut view = EditorView::with_text("abcdef\n");
        view.click(PointerTarget::Prefix { line: 2, col: 3 });
        view.handle_key(EditorKey::End);
        assert_eq!(view.prefix_col(), 3);
        assert!(!view.prefix_pending(2));

        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::Char('D'));
        view.handle_key(EditorKey::Char('D'));
        view.click(PointerTarget::Prefix { line: 2, col: 2 });
        view.handle_key(EditorKey::End);
        assert!(view.prefix_pending(2));
        assert_eq!(view.shown_prefix(2, "00000001"), "D       ");

        view.click(PointerTarget::Prefix { line: 2, col: 1 });
        view.handle_key(EditorKey::End);
        assert!(!view.prefix_pending(2));
        assert_eq!(view.prefix_col(), 1);

        view.click(PointerTarget::Command { col: 1 });
        for ch in "FIND".chars() {
            view.handle_key(EditorKey::Char(ch));
        }
        view.click(PointerTarget::Command { col: 3 });
        view.handle_key(EditorKey::End);
        assert_eq!(view.command(), "FI");
        assert_eq!(view.command_col(), 3);
        view.handle_key(EditorKey::End);
        assert_eq!(view.command(), "FI");
    }
}
