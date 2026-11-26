// SPDX-License-Identifier: GPL-3.0-or-later
//! Edit tabs. SPFLite kept one object per tab and a `TP` pointer at the
//! current one. A new config file comes later; tabs here live in memory.

use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::error::{EditError, EditResult};

#[derive(Clone)]
pub struct Tab {
    pub id: u32,
    pub title: String,
    pub path: Option<PathBuf>,
    pub doc: Document,
}

/// Close the current tab after a primary command runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingClose {
    /// CANCEL. Drop unsaved changes and close.
    Discard,
    /// EXIT. Save first, then close.
    Save,
}

pub struct Session {
    tabs: Vec<Tab>,
    current: usize,
    /// The screen SWAP would return to. Shown with '-' on the swap bar.
    alternate: Option<usize>,
    /// ISPF SWAPBAR. The list stays on the last line until it is turned off.
    swapbar: bool,
    /// The window should bring the current screen's cursor into view.
    pending_reveal: bool,
    /// SWAP LIST is waiting for a choice.
    swap_list: bool,
    /// CANCEL or EXIT asked the window to close this tab.
    pending_close: Option<PendingClose>,
    next_id: u32,
    clip: Vec<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            tabs: vec![Tab {
                id: 1,
                title: "Untitled".to_string(),
                path: None,
                doc: Document::new(),
            }],
            current: 0,
            alternate: None,
            swapbar: false,
            pending_reveal: false,
            swap_list: false,
            pending_close: None,
            next_id: 2,
            clip: Vec::new(),
        }
    }

    pub fn request_close(&mut self, kind: PendingClose) {
        self.pending_close = Some(kind);
    }

    pub fn take_pending_close(&mut self) -> Option<PendingClose> {
        self.pending_close.take()
    }

    pub fn set_clip(&mut self, lines: Vec<String>) {
        self.clip = lines;
    }

    pub fn clip(&self) -> &[String] {
        &self.clip
    }

    pub fn current(&self) -> &Tab {
        &self.tabs[self.current]
    }

    pub fn current_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.current]
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Remove the current tab. The last tab stays, and the method returns false.
    pub fn close_current(&mut self) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        let removed = self.current;
        self.tabs.remove(removed);
        if self.current >= self.tabs.len() {
            self.current = self.tabs.len() - 1;
        }
        self.alternate = match self.alternate {
            Some(alt) if alt == removed => None,
            Some(alt) if alt > removed => Some(alt - 1),
            other => other,
        };
        if self.alternate == Some(self.current) {
            self.alternate = None;
        }
        true
    }

    pub fn select(&mut self, index: usize) -> EditResult<()> {
        if index >= self.tabs.len() {
            return Err(EditError::new("No such tab"));
        }
        if index != self.current {
            self.alternate = Some(self.current);
            self.current = index;
        }
        Ok(())
    }

    /// Move the tab at `from` so it occupies `to`. The open file stays current.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        let len = self.tabs.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.current = shift_index(self.current, from, to);
        self.alternate = self
            .alternate
            .map(|index| shift_index(index, from, to))
            .filter(|&index| index != self.current);
    }

    pub fn swapbar(&self) -> bool {
        self.swapbar
    }

    pub fn set_swapbar(&mut self, on: bool) {
        self.swapbar = on;
    }

    pub fn toggle_swapbar(&mut self) -> bool {
        self.swapbar = !self.swapbar;
        self.swapbar
    }

    /// The screen a bare SWAP returns to, when it is still open.
    pub fn alternate(&self) -> Option<usize> {
        self.alternate
            .filter(|&index| index < self.tabs.len() && index != self.current)
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    /// First screen that already has this file, if any.
    pub fn index_of_path(&self, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.path.as_ref().is_some_and(|open| same_file(open, path)))
    }

    /// Switch to a screen and ask the window to show that screen's cursor.
    pub fn activate_screen(&mut self, index: usize) -> EditResult<()> {
        if index >= self.tabs.len() {
            return Err(EditError::new("No such tab"));
        }
        let moved = index != self.current;
        self.select(index)?;
        if moved {
            self.pending_reveal = true;
        }
        Ok(())
    }

    pub fn take_pending_reveal(&mut self) -> bool {
        std::mem::take(&mut self.pending_reveal)
    }

    pub fn swap_list_open(&self) -> bool {
        self.swap_list
    }

    pub fn set_swap_list(&mut self, open: bool) {
        self.swap_list = open;
    }

    /// ISPF "Start a new screen" from the task list.
    pub fn add_screen(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let title = fresh_title(&self.tabs);
        self.tabs.push(Tab {
            id,
            title,
            path: None,
            doc: Document::new(),
        });
        let index = self.tabs.len() - 1;
        self.alternate = Some(self.current);
        self.current = index;
        self.pending_reveal = true;
        index
    }

    pub fn tab_list(&self) -> String {
        self.tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let mark = if index == self.current { '*' } else { ' ' };
                let dirty = if tab.doc.is_modified() { "*" } else { "" };
                format!("{mark}{} {}{dirty}", index + 1, tab.title)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Load `path`. The untouched Untitled tab is reused; otherwise a new tab
    /// is opened and selected.
    pub fn open_path(&mut self, path: &Path) -> EditResult<()> {
        let doc = Document::load_path(path)?;
        let title = file_title(path);
        let replace = self.tabs.len() == 1
            && self.tabs[0].path.is_none()
            && !self.tabs[0].doc.is_modified()
            && self.tabs[0].doc.last_real() == 0;
        if replace {
            let id = self.tabs[0].id;
            self.tabs[0] = Tab {
                id,
                title,
                path: Some(path.to_path_buf()),
                doc,
            };
        } else {
            let id = self.next_id;
            self.next_id += 1;
            self.tabs.push(Tab {
                id,
                title,
                path: Some(path.to_path_buf()),
                doc,
            });
            self.alternate = Some(self.current);
            self.current = self.tabs.len() - 1;
        }
        Ok(())
    }

    /// Replace the current tab's text with the file on disk. The tab stays.
    pub fn reload_current(&mut self) -> EditResult<()> {
        let path = self.tabs[self.current]
            .path
            .clone()
            .ok_or_else(|| EditError::new("No file to re-open"))?;
        let doc = Document::load_path(&path)?;
        let title = file_title(&path);
        let tab = &mut self.tabs[self.current];
        tab.doc = doc;
        tab.title = title;
        Ok(())
    }

    pub fn save_current(&mut self, path: Option<&str>) -> EditResult<PathBuf> {
        let path = match path {
            Some(text) => PathBuf::from(text),
            None => self.tabs[self.current]
                .path
                .clone()
                .ok_or_else(|| EditError::new("No file name. Use SAVE path"))?,
        };
        self.tabs[self.current].doc.save_path(&path)?;
        self.tabs[self.current].title = file_title(&path);
        self.tabs[self.current].path = Some(path.clone());
        Ok(path)
    }
}

/// One SWAPBAR entry. `label` is `n:` plus the 8-column name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapField {
    pub index: usize,
    pub label: String,
}

/// Which part of the swap bar fits on this screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapWindow {
    pub from: usize,
    pub shown: usize,
    pub left: bool,
    pub right: bool,
}

const SWAP_NAME: usize = 8;

/// True when both paths name the same file. Comparison ignores case and slash style.
pub fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left), Ok(right)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        if left == right {
            return true;
        }
    }
    fold_path(left) == fold_path(right)
}

/// Index of a tab after the tab at `from` is inserted at `to`.
fn shift_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < index && index <= to {
        index - 1
    } else if to <= index && index < from {
        index + 1
    } else {
        index
    }
}

fn fold_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

/// Active screen starts with `*`, the alternate screen with `-`.
/// Each entry is `n:` plus that 8-column name.
pub fn swapbar_fields(names: &[&str], current: usize, alternate: Option<usize>) -> Vec<SwapField> {
    let count = names.len();
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let mark = if index == current {
                Some('*')
            } else if alternate == Some(index) {
                Some('-')
            } else {
                None
            };
            SwapField {
                index,
                label: swap_label(index, count, name, mark),
            }
        })
        .collect()
}

fn number_width(count: usize) -> usize {
    let mut n = count.max(1);
    let mut width = 1;
    while n >= 10 {
        width += 1;
        n /= 10;
    }
    width
}

fn swap_label(index: usize, count: usize, name: &str, mark: Option<char>) -> String {
    let digits = number_width(count);
    let mut prefix = format!("{}", index + 1);
    while prefix.chars().count() < digits {
        prefix.insert(0, ' ');
    }
    prefix.push(':');
    let mut body = String::new();
    let keep = if mark.is_some() { 7 } else { SWAP_NAME };
    if let Some(mark) = mark {
        body.push(mark);
    }
    body.extend(name.chars().take(keep));
    while body.chars().count() < SWAP_NAME {
        body.push(' ');
    }
    format!("{prefix}{body}")
}

fn field_width(count: usize) -> usize {
    number_width(count) + 1 + SWAP_NAME
}

/// How many entries fit. `<` and `>` take a column when the list continues.
pub fn swapbar_window(count: usize, from: usize, cols: usize) -> SwapWindow {
    if count == 0 || cols == 0 {
        return SwapWindow {
            from: 0,
            shown: 0,
            left: false,
            right: false,
        };
    }
    let from = from.min(count - 1);
    let left = from > 0;
    let left_cols = usize::from(left);
    let width = field_width(count);
    let stride = width + 1;
    let mut shown = entries_that_fit(cols.saturating_sub(left_cols), count - from, width, stride);
    let mut right = false;
    if from + shown < count {
        let with_mark = entries_that_fit(
            cols.saturating_sub(left_cols + 1),
            count - from,
            width,
            stride,
        );
        if with_mark > 0 {
            shown = with_mark;
            right = true;
        }
    }
    SwapWindow {
        from,
        shown,
        left,
        right,
    }
}

fn entries_that_fit(budget: usize, available: usize, width: usize, stride: usize) -> usize {
    if available == 0 || width == 0 || budget < width {
        return 0;
    }
    let packed = budget / stride;
    let tail = usize::from(budget % stride >= width);
    (packed + tail).min(available)
}

fn file_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn fresh_title(tabs: &[Tab]) -> String {
    for n in 1..1000 {
        let title = if n == 1 {
            "Untitled".to_string()
        } else {
            format!("Untitled{n}")
        };
        if !tabs
            .iter()
            .any(|tab| tab.title.eq_ignore_ascii_case(&title))
        {
            return title;
        }
    }
    "Screen".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swapbar_marks_the_active_and_alternate_screen() {
        let fields = swapbar_fields(&["PAYROLLS", "VERYLONGNAME", "A"], 0, Some(1));
        assert_eq!(fields[0].label, "1:*PAYROLL");
        assert_eq!(fields[1].label, "2:-VERYLON");
        assert_eq!(fields[2].label, "3:A       ");
        assert!(fields.iter().all(|field| field.label.chars().count() == 10));

        let wide = swapbar_window(2, 0, 80);
        assert_eq!(wide.shown, 2);
        assert!(!wide.left && !wide.right);

        let narrow = swapbar_window(6, 0, 20);
        assert_eq!(narrow.shown, 1);
        assert!(!narrow.left && narrow.right);

        let mid = swapbar_window(6, 2, 30);
        assert!(mid.left && mid.right);
        assert_eq!(mid.from, 2);
    }

    #[test]
    fn switching_screens_remembers_the_alternate() {
        let mut session = Session::new();
        session.tabs.push(Tab {
            id: 2,
            title: "SECOND".to_string(),
            path: None,
            doc: Document::new(),
        });
        session.next_id = 3;
        session.select(1).unwrap();
        assert_eq!(session.alternate(), Some(0));
        session.select(0).unwrap();
        assert_eq!(session.alternate(), Some(1));
        assert!(session.toggle_swapbar());
        session.set_swapbar(false);
        assert!(!session.swapbar());

        session.select(1).unwrap();
        assert!(session.close_current());
        assert_eq!(session.tabs().len(), 1);
        assert_eq!(session.alternate(), None);
    }

    #[test]
    fn dragging_a_tab_reorders_it_and_keeps_the_current_file() {
        let mut session = Session::new();
        session.tabs[0].title = "A".to_string();
        session.add_screen();
        session.tabs[1].title = "B".to_string();
        session.add_screen();
        session.tabs[2].title = "C".to_string();
        assert_eq!(session.current_index(), 2);
        assert_eq!(session.alternate(), Some(1));

        session.move_tab(0, 2);
        let titles: Vec<_> = session
            .tabs()
            .iter()
            .map(|tab| tab.title.as_str())
            .collect();
        assert_eq!(titles, ["B", "C", "A"]);
        assert_eq!(session.current().title, "C");
        assert_eq!(session.current_index(), 1);
        assert_eq!(session.alternate(), Some(0));

        session.move_tab(2, 0);
        let titles: Vec<_> = session
            .tabs()
            .iter()
            .map(|tab| tab.title.as_str())
            .collect();
        assert_eq!(titles, ["A", "B", "C"]);
        assert_eq!(session.current().title, "C");
        assert_eq!(session.current_index(), 2);
    }

    #[test]
    fn an_open_file_is_found_without_opening_it_again() {
        let path = std::env::temp_dir().join(format!("ispflite-focus-{}.txt", std::process::id()));
        std::fs::write(&path, "one\n").unwrap();
        let mut session = Session::new();
        session.open_path(&path).unwrap();
        session.add_screen();
        assert_eq!(session.current_index(), 1);
        assert_eq!(session.index_of_path(&path), Some(0));
        let slashed = PathBuf::from(path.to_string_lossy().replace('\\', "/"));
        assert!(same_file(&path, &slashed));
        assert_eq!(session.index_of_path(&slashed), Some(0));
        let _ = std::fs::remove_file(&path);
    }
}
