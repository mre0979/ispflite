// SPDX-License-Identifier: GPL-3.0-or-later
//! Exclude blocks.
//!
//! `X` marks data lines invisible. A following rebuild inserts one `XCLUDE`
//! marker in front of each run and stores the run length in `work1`. Hidden
//! lines store how far back the marker is, which is what `LPopOutX` uses.

use crate::document::Document;
use crate::error::{EditError, EditResult};
use crate::line::{Line, LineFlags};

impl Document {
    /// Mark data and note lines in `start..=end` invisible, then rebuild markers.
    pub fn exclude_range(&mut self, start: usize, end: usize) -> EditResult<usize> {
        self.ensure_span(start, end)?;
        let mut marked = 0usize;
        for line in &self.lines[start..=end] {
            if (line.is_data() || line.is_note()) && !line.is_invisible() {
                marked += 1;
            }
        }
        if marked == 0 {
            return Err(EditError::new("No lines to exclude"));
        }
        self.push_undo(false);
        for line in &mut self.lines[start..=end] {
            if line.is_data() || line.is_note() {
                line.flags.insert(LineFlags::INVISIBLE);
            }
        }
        self.rebuild_excludes();
        self.renumber();
        self.reveal_cursor_if_hidden();
        Ok(marked)
    }

    /// Hide every data or note line in `lines`, then rebuild the markers.
    pub fn exclude_lines(&mut self, lines: &[usize]) -> EditResult<usize> {
        let mut marked = 0usize;
        for &line in lines {
            self.ensure_line_pub(line)?;
            let row = &self.lines[line];
            if (row.is_data() || row.is_note()) && !row.is_invisible() {
                marked += 1;
            }
        }
        if marked == 0 {
            return Err(EditError::new("No lines to exclude"));
        }
        self.push_undo(false);
        for &line in lines {
            if self.lines[line].is_data() || self.lines[line].is_note() {
                self.lines[line].flags.insert(LineFlags::INVISIBLE);
            }
        }
        self.rebuild_excludes();
        self.renumber();
        self.reveal_cursor_if_hidden();
        Ok(marked)
    }

    fn ensure_line_pub(&self, n: usize) -> EditResult<()> {
        if n == 0 || n >= self.lines.len() {
            return Err(EditError::new(format!("Line {n} is not in the file")));
        }
        Ok(())
    }

    /// Make every hidden line visible and drop the markers.
    pub fn include_all(&mut self) -> EditResult<usize> {
        let hidden = self.lines.iter().filter(|line| line.is_invisible()).count();
        if hidden == 0 {
            return Err(EditError::new("No excluded lines"));
        }
        self.push_undo(false);
        for line in &mut self.lines {
            line.flags.remove(LineFlags::INVISIBLE);
            if line.is_xclude() {
                continue;
            }
            line.work1 = 0;
        }
        self.rebuild_excludes();
        self.renumber();
        Ok(hidden)
    }

    /// Show the first `count` lines of the exclude block that starts at `marker`.
    pub fn show_first(&mut self, marker: usize, count: usize) -> EditResult<()> {
        self.require_marker(marker)?;
        if count == 0 {
            return Err(EditError::new("Count is zero"));
        }
        self.push_undo(false);
        let have = self.lines[marker].work1.max(0) as usize;
        if count >= have {
            self.reveal_whole_block(marker, have);
        } else {
            self.lines[marker].work1 -= count as i32;
            self.pop_front(marker, count);
        }
        self.rebuild_excludes();
        self.renumber();
        Ok(())
    }

    /// Show the last `count` lines of the exclude block that starts at `marker`.
    pub fn show_last(&mut self, marker: usize, count: usize) -> EditResult<()> {
        self.require_marker(marker)?;
        if count == 0 {
            return Err(EditError::new("Count is zero"));
        }
        self.push_undo(false);
        let have = self.lines[marker].work1.max(0) as usize;
        if count >= have {
            self.reveal_whole_block(marker, have);
        } else {
            let from = marker + have - count + 1;
            let to = marker + have;
            for line in &mut self.lines[from..=to] {
                line.flags.remove(LineFlags::INVISIBLE);
                line.work1 = 0;
            }
            self.lines[marker].work1 -= count as i32;
        }
        self.rebuild_excludes();
        self.renumber();
        Ok(())
    }

    /// Pull one hidden line out of its exclude block. Returns its new index.
    pub fn pop_out(&mut self, line: usize) -> EditResult<usize> {
        let (marker, _) = self.locate_hidden(line)?;
        self.push_undo(false);
        self.pop_out_at(marker, line)
    }

    /// Same as [`Document::pop_out`] for a caller that already recorded undo.
    pub(crate) fn pop_out_silent(&mut self, line: usize) -> EditResult<usize> {
        let (marker, _) = self.locate_hidden(line)?;
        self.pop_out_at(marker, line)
    }

    pub(crate) fn rebuild_excludes(&mut self) {
        let mut i = 1usize;
        loop {
            let last = self.lines.len() - 1;
            if i > last {
                break;
            }
            if self.lines[i].is_xclude() {
                let next = i + 1;
                if next > last || !self.lines[next].is_invisible() {
                    self.lines.remove(i);
                    continue;
                }
                self.lines[i].work1 = 0;
                let mut j = i + 1;
                loop {
                    let last = self.lines.len() - 1;
                    if j > last {
                        break;
                    }
                    if self.lines[j].is_xclude() {
                        self.lines.remove(j);
                        continue;
                    }
                    if !self.lines[j].is_invisible() {
                        break;
                    }
                    let count = self.lines[i].work1 + 1;
                    self.lines[i].work1 = count;
                    self.lines[j].work1 = (j - i) as i32;
                    j += 1;
                }
                i = j;
            } else if self.lines[i].is_invisible() {
                let marker = Line::xclude(self.alloc_track());
                self.lines.insert(i, marker);
            } else {
                i += 1;
            }
        }
    }

    fn reveal_whole_block(&mut self, marker: usize, count: usize) {
        self.lines.remove(marker);
        for line in &mut self.lines[marker..marker + count] {
            line.flags.remove(LineFlags::INVISIBLE);
            line.work1 = 0;
        }
    }

    /// Move `count` hidden lines out from just after the marker. The marker's
    /// `work1` has already been reduced to the number that stays hidden.
    fn pop_front(&mut self, marker: usize, count: usize) {
        let saved = self.lines[marker].clone();
        self.lines.remove(marker);
        for line in &mut self.lines[marker..marker + count] {
            line.flags.remove(LineFlags::INVISIBLE);
            line.flags.insert(LineFlags::POPPED);
            line.work1 = 0;
        }
        self.lines.insert(marker + count, saved);
        let remain = self.lines[marker + count].work1.max(0) as usize;
        for j in 1..=remain {
            self.lines[marker + count + j].work1 = j as i32;
        }
    }

    fn pop_one(&mut self, marker: usize, line: usize) -> EditResult<usize> {
        let have = self.lines[marker].work1.max(0) as usize;
        let back = self.lines[line].work1.max(0) as usize;
        let last_hidden = marker + have;
        if have == 1 {
            self.lines.remove(marker);
            self.lines[marker].flags.remove(LineFlags::INVISIBLE);
            self.lines[marker].flags.insert(LineFlags::POPPED);
            self.lines[marker].work1 = 0;
            return Ok(marker);
        }
        if line == last_hidden {
            self.lines[line].flags.remove(LineFlags::INVISIBLE);
            self.lines[line].flags.insert(LineFlags::POPPED);
            self.lines[line].work1 = 0;
            self.lines[marker].work1 -= 1;
            return Ok(line);
        }
        if back == 1 {
            self.lines[marker].work1 -= 1;
            let saved = self.lines[marker].clone();
            self.lines.remove(marker);
            self.lines[marker].flags.remove(LineFlags::INVISIBLE);
            self.lines[marker].flags.insert(LineFlags::POPPED);
            self.lines[marker].work1 = 0;
            self.lines.insert(marker + 1, saved);
            let remain = self.lines[marker + 1].work1.max(0) as usize;
            for j in 1..=remain {
                self.lines[marker + 1 + j].work1 = j as i32;
            }
            return Ok(marker);
        }
        let lower = have - back;
        self.lines[marker].work1 = (back as i32) - 1;
        self.lines[line].flags.remove(LineFlags::INVISIBLE);
        self.lines[line].flags.insert(LineFlags::POPPED);
        self.lines[line].work1 = 0;
        let mut fresh = Line::xclude(self.alloc_track());
        fresh.work1 = lower as i32;
        self.lines.insert(line + 1, fresh);
        for j in 1..=lower {
            self.lines[line + 1 + j].work1 = j as i32;
        }
        Ok(line)
    }

    fn locate_hidden(&self, line: usize) -> EditResult<(usize, usize)> {
        if !self.line(line)?.is_invisible() {
            return Err(EditError::new("Line is not excluded"));
        }
        let back = self.lines[line].work1;
        if back <= 0 {
            return Err(EditError::new("Excluded line is missing its marker"));
        }
        let marker = line - back as usize;
        if marker < 1 || !self.lines[marker].is_xclude() {
            return Err(EditError::new("Exclude marker not found"));
        }
        Ok((marker, line))
    }

    fn pop_out_at(&mut self, marker: usize, line: usize) -> EditResult<usize> {
        let shown = self.pop_one(marker, line)?;
        self.rebuild_excludes();
        self.renumber();
        Ok(shown)
    }

    pub(crate) fn reveal_cursor_if_hidden(&mut self) {
        let current = self.cursor().0;
        if self
            .line(current)
            .map(|line| line.is_invisible())
            .unwrap_or(false)
        {
            let back = self.lines[current].work1.max(0) as usize;
            if back > 0 && current >= back {
                let _ = self.set_cursor(current - back, 1);
                return;
            }
        }
        let _ = self.set_cursor(current, self.cursor().1);
    }

    fn require_marker(&self, marker: usize) -> EditResult<()> {
        if self.line(marker)?.is_xclude() {
            Ok(())
        } else {
            Err(EditError::new("Command is only valid on an excluded line"))
        }
    }

    fn ensure_span(&self, start: usize, end: usize) -> EditResult<()> {
        if start == 0 || end < start || end > self.last_line() {
            return Err(EditError::new("Line range is outside the file"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Document {
        Document::from_text("a\nb\nc\nd\ne\n")
    }

    fn texts(doc: &Document) -> Vec<String> {
        (1..=doc.last_line())
            .map(|i| {
                let line = doc.line(i).unwrap();
                let mark = if line.is_invisible() {
                    "h"
                } else if line.is_xclude() {
                    "X"
                } else if line.is_data() {
                    "d"
                } else {
                    "s"
                };
                format!("{mark}:{}", line.text())
            })
            .collect()
    }

    #[test]
    fn exclude_builds_one_marker() {
        let mut doc = sample();
        // b c d are sequence lines 2..=4, raw indexes 3..=5.
        assert_eq!(doc.exclude_range(3, 5).unwrap(), 3);
        assert_eq!(
            texts(&doc),
            vec![
                "s:Top of Data",
                "d:a",
                "X:",
                "h:b",
                "h:c",
                "h:d",
                "d:e",
                "s:Bottom of Data",
            ]
        );
        assert_eq!(doc.line(3).unwrap().work1, 3);
        assert_eq!(doc.line(4).unwrap().work1, 1);
        assert_eq!(doc.line(6).unwrap().work1, 3);
        let visible: Vec<_> = doc
            .visible_rows()
            .iter()
            .map(|row| row.text.clone())
            .collect();
        assert_eq!(visible, vec!["Top of Data", "a", "", "e", "Bottom of Data"]);
    }

    #[test]
    fn show_first_and_last_and_include() {
        let mut doc = sample();
        doc.exclude_range(3, 5).unwrap();
        doc.show_first(3, 1).unwrap();
        assert_eq!(doc.line_text(3).unwrap(), "b");
        assert!(doc.line(4).unwrap().is_xclude());
        assert_eq!(doc.line(4).unwrap().work1, 2);
        assert_eq!(doc.line_text(5).unwrap(), "c");
        assert!(doc.line(5).unwrap().is_invisible());

        doc.show_last(4, 1).unwrap();
        assert!(doc.line(4).unwrap().is_xclude());
        assert_eq!(doc.line(4).unwrap().work1, 1);
        assert_eq!(doc.line_text(5).unwrap(), "c");
        assert!(doc.line(5).unwrap().is_invisible());
        assert_eq!(doc.line_text(6).unwrap(), "d");
        assert!(!doc.line(6).unwrap().is_invisible());

        doc.include_all().unwrap();
        assert_eq!(doc.to_text(), "a\nb\nc\nd\ne\n");
        assert_eq!(doc.last_line(), 7);
    }

    #[test]
    fn pop_four_positions() {
        let mut doc = sample();
        doc.exclude_range(3, 5).unwrap();
        // Single-line block: exclude only c.
        let mut one = sample();
        one.exclude_range(4, 4).unwrap();
        // The marker is inserted in front of c, so c moves from 4 to 5.
        let shown = one.pop_out(5).unwrap();
        assert_eq!(shown, 4);
        assert_eq!(one.line_text(shown).unwrap(), "c");
        assert!(!one.line(shown).unwrap().is_invisible());
        assert_eq!(one.last_real(), 5);

        // First line of a block.
        let at = doc.pop_out(4).unwrap();
        assert_eq!(doc.line_text(at).unwrap(), "b");
        assert!(doc.line(at + 1).unwrap().is_xclude());
        assert_eq!(doc.line(at + 1).unwrap().work1, 2);

        // Last line of what remains (c hidden, d last).
        let marker = at + 1;
        let last = marker + doc.line(marker).unwrap().work1 as usize;
        assert_eq!(doc.line_text(last).unwrap(), "d");
        let shown = doc.pop_out(last).unwrap();
        assert_eq!(doc.line_text(shown).unwrap(), "d");
        assert!(!doc.line(shown).unwrap().is_invisible());

        // Middle of a fresh 3-line block.
        let mut mid = sample();
        mid.exclude_range(3, 5).unwrap();
        let shown = mid.pop_out(5).unwrap();
        assert_eq!(mid.line_text(shown).unwrap(), "c");
        assert!(mid.line(shown - 1).unwrap().is_invisible());
        assert!(mid.line(shown + 1).unwrap().is_xclude());
        assert_eq!(mid.line_text(shown + 2).unwrap(), "d");
        assert!(mid.line(shown + 2).unwrap().is_invisible());
        assert_eq!(mid.line(3).unwrap().work1, 1);
        assert_eq!(mid.line(shown + 1).unwrap().work1, 1);
    }
}
