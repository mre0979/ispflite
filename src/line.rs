// SPDX-License-Identifier: GPL-3.0-or-later
//! One row in an edit session.
//!
//! The line owns its text. Flag values match the original editor so command
//! tests can keep the same bit checks.

/// Bits from `DataLine.LFlag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineFlags(pub u32);

impl LineFlags {
    pub const DATA: u32 = 0x0000_0001;
    pub const TOP: u32 = 0x0000_0002;
    pub const BOTTOM: u32 = 0x0000_0004;
    pub const TABS: u32 = 0x0000_0008;
    pub const BOUNDS: u32 = 0x0000_0010;
    pub const COLS: u32 = 0x0000_0020;
    pub const XCLUDE: u32 = 0x0000_0040;
    pub const INVISIBLE: u32 = 0x0000_0080;
    pub const INSERT: u32 = 0x0000_0100;
    pub const EQ_CHANGE: u32 = 0x0000_0200;
    pub const WORD: u32 = 0x0000_0400;
    pub const MARK: u32 = 0x0000_0800;
    pub const MASK: u32 = 0x0000_1000;
    pub const PROF: u32 = 0x0000_2000;
    pub const CURSOR: u32 = 0x0000_4000;
    pub const TOP_SCREEN: u32 = 0x0000_8000;
    pub const SCROLL: u32 = 0x0001_0000;
    pub const XPTR: u32 = 0x0002_0000;
    pub const PAGE: u32 = 0x0004_0000;
    pub const FILE: u32 = 0x0008_0000;
    pub const NOTE: u32 = 0x0010_0000;
    pub const POPPED: u32 = 0x0020_0000;
    pub const USER: u32 = 0x0040_0000;
    pub const EFT: u32 = 0x0080_0000;
    pub const PANEL_UPPER: u32 = 0x0100_0000;
    pub const PANEL_LOWER: u32 = 0x0200_0000;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    pub fn insert(&mut self, bit: u32) {
        self.0 |= bit;
    }

    pub fn remove(&mut self, bit: u32) {
        self.0 &= !bit;
    }

    pub fn is_numbered(self) -> bool {
        self.contains(Self::DATA) || self.contains(Self::NOTE)
    }
}

/// A single edit line. Columns are Unicode scalar values, 1-based, which is
/// the same numbering `MID$` used for byte columns on ANSI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub flags: LineFlags,
    text: String,
    /// One attribute word per column. Zero is the default text scheme.
    attr: Vec<u16>,
    /// Dot label, uppercase, including the leading `.`. Absent when unset.
    pub label: Option<String>,
    /// Colon tag, uppercase, including the leading `:`.
    pub tag: Option<String>,
    /// Sequence number shown in the prefix. Zero on special lines.
    pub seq: u32,
    /// Exclude count on a marker, or the distance back to the marker on a
    /// hidden line. This is `LWrk1`.
    pub work1: i32,
    pub work2: i32,
    pub find_ix: i32,
    pub track: i32,
    pub handle: i32,
    pub mix: i32,
    pub col: i32,
}

impl Line {
    pub(crate) fn blank(flags: u32, track: i32) -> Self {
        Self {
            flags: LineFlags(flags),
            text: String::new(),
            attr: Vec::new(),
            label: None,
            tag: None,
            seq: 0,
            work1: 0,
            work2: 0,
            find_ix: 0,
            track,
            handle: 0,
            mix: 0,
            col: 0,
        }
    }

    pub(crate) fn top(track: i32) -> Self {
        let mut line = Self::blank(LineFlags::TOP, track);
        line.text = "Top of Data".to_string();
        line.sync_attr();
        line
    }

    pub(crate) fn bottom(track: i32) -> Self {
        let mut line = Self::blank(LineFlags::BOTTOM, track);
        line.text = "Bottom of Data".to_string();
        line.sync_attr();
        line
    }

    pub(crate) fn data(track: i32) -> Self {
        Self::blank(LineFlags::DATA, track)
    }

    pub(crate) fn xclude(track: i32) -> Self {
        Self::blank(LineFlags::XCLUDE, track)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn attrs(&self) -> &[u16] {
        &self.attr
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    pub(crate) fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.sync_attr();
    }

    pub(crate) fn set_chars(&mut self, chars: &[char]) {
        self.text = chars.iter().collect();
        self.sync_attr();
    }

    fn sync_attr(&mut self) {
        let n = self.char_len();
        if self.attr.len() != n {
            self.attr.resize(n, 0);
        }
    }

    pub(crate) fn pad_to(&mut self, min_len: usize) {
        let n = self.char_len();
        if min_len > n {
            self.text.push_str(&" ".repeat(min_len - n));
            self.attr.resize(min_len, 0);
        }
    }

    pub fn is_top(&self) -> bool {
        self.flags.contains(LineFlags::TOP)
    }

    pub fn is_bottom(&self) -> bool {
        self.flags.contains(LineFlags::BOTTOM)
    }

    pub fn is_data(&self) -> bool {
        self.flags.contains(LineFlags::DATA)
    }

    pub fn is_note(&self) -> bool {
        self.flags.contains(LineFlags::NOTE)
    }

    pub fn is_xclude(&self) -> bool {
        self.flags.contains(LineFlags::XCLUDE)
    }

    pub fn is_invisible(&self) -> bool {
        self.flags.contains(LineFlags::INVISIBLE)
    }

    pub fn is_insert(&self) -> bool {
        self.flags.contains(LineFlags::INSERT)
    }

    /// Eight-character prefix area. An `I` line keeps quotes while its text
    /// is being entered, matching `LLCtl = "''''''''"`.
    pub fn display_prefix(&self) -> String {
        if self.is_top() || self.is_bottom() {
            return "********".to_string();
        }
        if self.is_xclude() {
            return format!("X{:>7}", self.work1.max(0));
        }
        if self.flags.contains(LineFlags::COLS) {
            return fit_prefix("=COLS>");
        }
        if self.flags.contains(LineFlags::TABS) {
            return fit_prefix("=TABS>");
        }
        if self.flags.contains(LineFlags::BOUNDS) {
            return fit_prefix("=BNDS>");
        }
        if self.flags.contains(LineFlags::MASK) {
            return fit_prefix("=MASK>");
        }
        if self.flags.contains(LineFlags::WORD) {
            return fit_prefix("=WORD>");
        }
        if self.flags.contains(LineFlags::MARK) {
            return fit_prefix("=MARK>");
        }
        if self.is_note() {
            return fit_prefix("=NOTE>");
        }
        if self.flags.contains(LineFlags::USER) {
            return fit_prefix("=USER>");
        }
        if let Some(label) = &self.label {
            return fit_prefix(label);
        }
        if self.flags.contains(LineFlags::EQ_CHANGE) {
            return fit_prefix("==CHG>");
        }
        if self.is_insert() {
            return "''''''''".to_string();
        }
        if self.seq > 0 {
            return format!("{:08}", self.seq);
        }
        "        ".to_string()
    }
}

fn fit_prefix(text: &str) -> String {
    let mut out: String = text.chars().take(8).collect();
    while out.chars().count() < 8 {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_bits_keep_their_values() {
        assert_eq!(LineFlags::DATA, 0x1);
        assert_eq!(LineFlags::XCLUDE, 0x40);
        assert_eq!(LineFlags::INVISIBLE, 0x80);
        assert_eq!(LineFlags::EQ_CHANGE, 0x200);
        assert_eq!(LineFlags::NOTE, 0x10_0000);
    }

    #[test]
    fn columns_follow_chars_not_bytes() {
        let mut line = Line::data(1);
        line.set_text("café");
        assert_eq!(line.char_len(), 4);
        assert_eq!(line.attrs().len(), 4);
        line.pad_to(6);
        assert_eq!(line.text(), "café  ");
    }
}
