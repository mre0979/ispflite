// SPDX-License-Identifier: GPL-3.0-or-later
//! Primary commands and line commands that sit on top of the document model:
//! repeated searches, joins, overlays, the clipboard, and file create/replace.

use std::fs;
use std::path::Path;

use crate::document::{pair_name, DestKind, Document, LineShift, PairKind};
use crate::error::{EditError, EditResult};
use crate::line::LineFlags;
use crate::session::Session;

pub(crate) fn rfind(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (text, case_sensitive, qualifiers) = doc
        .remembered_find()
        .ok_or_else(|| EditError::new("No previous FIND"))?;
    let upward = match rest.trim().to_ascii_uppercase().as_str() {
        "" | "NEXT" | "N" => false,
        "PREV" | "PREVIOUS" | "P" => true,
        other => return Err(EditError::new(format!("Unknown keyword '{other}'"))),
    };
    crate::commands::find_again(doc, &text, case_sensitive, &qualifiers, upward)
}

pub(crate) fn rchange(doc: &mut Document) -> EditResult<String> {
    let (from, case_sensitive, to, qualifiers) = doc
        .remembered_change()
        .ok_or_else(|| EditError::new("No previous CHANGE"))?;
    crate::commands::change_again(doc, &from, case_sensitive, &to, &qualifiers)
}

pub(crate) fn rlocate(doc: &mut Document) -> EditResult<String> {
    let operand = doc
        .remembered_locate()
        .ok_or_else(|| EditError::new("No previous LOCATE"))?;
    crate::commands::locate_again(doc, &operand)
}

pub(crate) fn nfind(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (text, case_sensitive) = search_text(rest)?;
    let start = doc.cursor().0;
    let mut line = start.max(1);
    let mut wrapped = false;
    loop {
        if (doc.line(line)?.is_data() || doc.line(line)?.is_note())
            && !contains(doc.line_text(line)?, &text, case_sensitive)
        {
            doc.set_cursor(line, 1)?;
            let seq = doc.line(line)?.seq;
            return Ok(format!("Not found on line {seq}"));
        }
        line += 1;
        if line > doc.last_line() {
            if wrapped {
                break;
            }
            wrapped = true;
            line = 1;
        }
        if wrapped && line > start {
            break;
        }
    }
    Err(EditError::new(format!("Every line contains '{text}'")))
}

pub(crate) fn nexclude(doc: &mut Document, rest: &str) -> EditResult<String> {
    let lines = lines_without(doc, rest)?;
    let n = doc.exclude_lines(&lines)?;
    Ok(format!("Excluded {n} line{}", plural(n)))
}

pub(crate) fn ndelete(doc: &mut Document, rest: &str) -> EditResult<String> {
    let lines = lines_without(doc, rest)?;
    let n = doc.delete_lines(&lines)?;
    Ok(format!("Deleted {n} line{}", plural(n)))
}

pub(crate) fn flip(doc: &mut Document, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("FLIP takes no operands"));
    }
    let n = doc.flip()?;
    Ok(format!("Flipped {n} line{}", plural(n)))
}

pub(crate) fn show(doc: &mut Document, rest: &str) -> EditResult<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        let n = doc.include_all()?;
        return Ok(format!("Shown {n} line{}", plural(n)));
    }
    let (text, case_sensitive) = search_text(rest)?;
    let n = doc.show_matching(&text, case_sensitive)?;
    Ok(format!("Shown {n} line{}", plural(n)))
}

pub(crate) fn hide(doc: &mut Document, rest: &str) -> EditResult<String> {
    let rest = rest.trim();
    let n = if rest.is_empty() {
        doc.hide_lines(None, false)?
    } else {
        let (text, case_sensitive) = search_text(rest)?;
        doc.hide_lines(Some(&text), case_sensitive)?
    };
    Ok(format!("Hidden {n} line{}", plural(n)))
}

pub(crate) fn cut(session: &mut Session, rest: &str) -> EditResult<String> {
    use crate::document::PendingBlock;
    let pending = session.current().doc.pending();
    if matches!(
        pending,
        PendingBlock::After { .. } | PendingBlock::Before { .. }
    ) {
        return Err(EditError::new("command conflict"));
    }
    let copied = match pending {
        PendingBlock::CopyLine { line, lines } => {
            let end = session.current().doc.span_end(line, lines.max(1))?;
            Some((line, end))
        }
        PendingBlock::CopySpan { start, end, .. } => Some((start, end)),
        PendingBlock::CopyOpen { .. } => {
            return Err(EditError::new("CC block is still open"));
        }
        _ => None,
    };
    let (start, end) = if let Some(range) = copied {
        range
    } else if rest.trim().is_empty() {
        if session.current().doc.last_real() == 0 {
            return Err(EditError::new("No lines to copy"));
        }
        (2, session.current().doc.last_line() - 1)
    } else {
        let doc = &session.current().doc;
        clip_range(doc, rest)?
    };
    let lines = collect_text(&session.current().doc, start, end)?;
    let n = lines.len();
    session.set_clip(lines);
    if copied.is_some() {
        session.current_mut().doc.set_pending(PendingBlock::None);
    }
    Ok(format!("Copied {n} line{}", plural(n)))
}

pub(crate) fn clip(session: &mut Session, rest: &str) -> EditResult<String> {
    let doc = &session.current().doc;
    let (start, end) = clip_range(doc, rest)?;
    let lines = collect_text(doc, start, end)?;
    let n = lines.len();
    session.set_clip(lines);
    Ok(format!("Clipped {n} line{}", plural(n)))
}

pub(crate) fn paste(session: &mut Session, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("PASTE takes no operands"));
    }
    let lines = session.clip().to_vec();
    if lines.is_empty() {
        return Err(EditError::new("The clipboard is empty"));
    }
    let after = session.current().doc.cursor().0;
    let n = lines.len();
    session.current_mut().doc.insert_text_lines(after, &lines)?;
    Ok(format!("Pasted {n} line{}", plural(n)))
}

pub(crate) fn copy_file(doc: &mut Document, rest: &str) -> EditResult<String> {
    let path = need_path(rest, "COPY")?;
    let loaded = Document::load_path(Path::new(&path))?;
    let lines: Vec<String> = (1..loaded.last_line())
        .filter_map(|line| {
            let row = loaded.line(line).ok()?;
            if row.is_data() {
                Some(row.text().to_string())
            } else {
                None
            }
        })
        .collect();
    let n = lines.len();
    doc.insert_text_lines(doc.cursor().0, &lines)?;
    Ok(format!("Copied {n} line{}", plural(n)))
}

pub(crate) fn create_file(doc: &Document, rest: &str) -> EditResult<String> {
    let path = need_path(rest, "CREATE")?;
    if Path::new(&path).exists() {
        return Err(EditError::new("File already exists. Use REPLACE"));
    }
    write_data(doc, &path)?;
    Ok(format!("Created {path}"))
}

pub(crate) fn replace_file(doc: &Document, rest: &str) -> EditResult<String> {
    let path = need_path(rest, "REPLACE")?;
    write_data(doc, &path)?;
    Ok(format!("Replaced {path}"))
}

pub(crate) fn cancel(session: &mut Session, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("CANCEL takes no operands"));
    }
    session.request_close(crate::session::PendingClose::Discard);
    Ok("Cancelled".to_string())
}

pub(crate) fn exit_cmd(session: &mut Session, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("EXIT takes no operands"));
    }
    session.request_close(crate::session::PendingClose::Save);
    Ok("Exiting".to_string())
}

pub(crate) fn rename(session: &mut Session, rest: &str) -> EditResult<String> {
    let path = need_path(rest, "RENAME")?;
    let old = session
        .current()
        .path
        .clone()
        .ok_or_else(|| EditError::new("Save the file before RENAME"))?;
    let new = Path::new(&path);
    fs::rename(&old, new).map_err(|err| EditError::new(format!("Cannot rename: {err}")))?;
    session.current_mut().title = new
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    session.current_mut().path = Some(new.to_path_buf());
    Ok(format!("Renamed to {path}"))
}

pub(crate) fn join_at_cursor(doc: &mut Document, rest: &str) -> EditResult<String> {
    let count = optional_count(rest, "JOIN")?.unwrap_or(1);
    let start = data_line(doc, doc.cursor().0)?;
    let end = doc.span_end(start, count + 1)?;
    doc.join_range(start, end, " ", true)?;
    Ok("Joined".to_string())
}

pub(crate) fn split_at_cursor(doc: &mut Document, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("SPLIT takes no operands"));
    }
    let (line, col) = doc.cursor();
    doc.split_line(line, col)?;
    Ok("Split".to_string())
}

pub(crate) fn append_text(doc: &mut Document, rest: &str) -> EditResult<String> {
    let text = rest;
    if text.is_empty() {
        return Err(EditError::new("APPEND needs text"));
    }
    let line = data_line(doc, doc.cursor().0)?;
    let mut next = doc.line_text(line)?.to_string();
    next.push_str(text);
    doc.set_line_text(line, &next)?;
    Ok("Appended".to_string())
}

pub(crate) fn prepend_text(doc: &mut Document, rest: &str) -> EditResult<String> {
    if rest.is_empty() {
        return Err(EditError::new("PREPEND needs text"));
    }
    let line = data_line(doc, doc.cursor().0)?;
    let next = format!("{rest}{}", doc.line_text(line)?);
    doc.set_line_text(line, &next)?;
    Ok("Prepended".to_string())
}

pub(crate) fn align(doc: &mut Document, rest: &str) -> EditResult<String> {
    let column = optional_count(rest, "ALIGN")?.unwrap_or(1).max(1);
    let last = doc.last_line();
    doc.map_span(2, last.saturating_sub(1).max(2), |text| {
        let trimmed = text.trim_start();
        let pad = column - 1;
        format!("{}{trimmed}", " ".repeat(pad))
    })?;
    Ok(format!("Aligned at column {column}"))
}

pub(crate) fn compress(doc: &mut Document, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("COMPRESS takes no operands"));
    }
    let last = doc.last_line();
    doc.map_span(2, last.saturating_sub(1).max(2), squeeze)?;
    Ok("Compressed".to_string())
}

pub(crate) fn case_all(doc: &mut Document, rest: &str, kind: CaseKind) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("That command takes no operands"));
    }
    let last = doc.last_line();
    doc.map_span(2, last.saturating_sub(1).max(2), |text| {
        case_text(text, kind)
    })?;
    Ok(match kind {
        CaseKind::Upper => "Uppercased".to_string(),
        CaseKind::Lower => "Lowercased".to_string(),
        CaseKind::Sentence => "Sentence case".to_string(),
        CaseKind::Title => "Title case".to_string(),
    })
}

pub(crate) fn line_number(doc: &mut Document, rest: &str) -> EditResult<String> {
    crate::commands::locate_again(doc, rest)
}

pub(crate) fn cols(doc: &mut Document, rest: &str) -> EditResult<String> {
    let on = match rest.trim().to_ascii_uppercase().as_str() {
        "" => !doc.cols_mode(),
        "ON" => true,
        "OFF" => false,
        other => return Err(EditError::new(format!("Unknown keyword '{other}'"))),
    };
    doc.set_cols(on);
    Ok(format!("COLS set to {}", if on { "ON" } else { "OFF" }))
}

pub(crate) fn hex(_doc: &mut Document, _rest: &str) -> EditResult<String> {
    Err(EditError::new("Not supported"))
}

/// ISPF RENUM turns number mode on and numbers every line from 100 by 100.
/// STD numbers stay in the line-number column. COBOL also writes a 6-digit
/// number over columns 1–6. DISPLAY is accepted; the numbers are already shown.
pub(crate) fn renum(doc: &mut Document, rest: &str) -> EditResult<String> {
    let mut on = false;
    let mut off = false;
    let mut std = false;
    let mut cobol = false;
    let mut display = false;
    for token in rest.split_whitespace() {
        match token.to_ascii_uppercase().as_str() {
            "ON" => on = true,
            "OFF" => off = true,
            "STD" => std = true,
            "COBOL" => cobol = true,
            "DISPLAY" => display = true,
            other => return Err(EditError::new(format!("Unknown keyword '{other}'"))),
        }
    }
    if off && (on || std || cobol || display) {
        return Err(EditError::new("command conflict"));
    }
    if off {
        doc.push_undo(false);
        doc.set_number_mode(false, false, false, false);
        doc.renumber();
        return Ok("Number mode off".to_string());
    }
    if !std && !cobol {
        std = true;
    }
    doc.push_undo(cobol);
    doc.set_number_mode(true, std, cobol, display);
    doc.renumber();
    let kind = doc.number_kind().unwrap_or("STD");
    let shown = if doc.number_display() {
        format!("{kind} DISPLAY")
    } else {
        kind.to_string()
    };
    Ok(format!("Renumbered {shown}"))
}

pub(crate) fn move_cursor(
    doc: &mut Document,
    rest: &str,
    down: bool,
    vertical: bool,
) -> EditResult<String> {
    let count = optional_count(rest, "Scroll")?.unwrap_or(1).max(1);
    let (line, col) = doc.cursor();
    if vertical {
        let rows = doc.visible_rows();
        let idx = rows.iter().position(|row| row.line == line).unwrap_or(0);
        let next = if down {
            idx.saturating_add(count)
        } else {
            idx.saturating_sub(count)
        };
        let next = next.min(rows.len().saturating_sub(1));
        doc.set_cursor(rows[next].line, col)?;
        Ok(if down {
            "Down".to_string()
        } else {
            "Up".to_string()
        })
    } else {
        let next = if down {
            col.saturating_add(count)
        } else {
            col.saturating_sub(count).max(1)
        };
        doc.set_cursor(line, next)?;
        Ok(if down {
            "Right".to_string()
        } else {
            "Left".to_string()
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CaseKind {
    Upper,
    Lower,
    Sentence,
    Title,
}

pub(crate) fn line_action(
    doc: &mut Document,
    line: usize,
    kind: LineOp,
    block: bool,
    repeat: usize,
) -> EditResult<String> {
    if block {
        return block_pair(doc, line, kind, repeat);
    }
    match kind {
        LineOp::Join => join_lines(doc, line, repeat.max(1), " ", false),
        LineOp::Glue => join_lines(doc, line, repeat.max(1), "", false),
        LineOp::TextJoin => join_lines(doc, line, repeat.max(1), " ", true),
        LineOp::TextGlue => join_lines(doc, line, repeat.max(1), "", true),
        LineOp::Sentence => map_count(doc, line, repeat.max(1), CaseKind::Sentence),
        LineOp::Title => map_count(doc, line, repeat.max(1), CaseKind::Title),
        LineOp::Trim => map_count_text(
            doc,
            line,
            repeat.max(1),
            |text| text.trim_end().to_string(),
            "Trimmed",
        ),
        LineOp::Pad => {
            let width = if repeat == 0 { 80 } else { repeat };
            map_count_text(doc, line, 1, |text| pad_line(text, width), "Padded")
        }
        LineOp::Trunc => {
            let width = if repeat == 0 { 80 } else { repeat };
            map_count_text(doc, line, 1, |text| trunc_line(text, width), "Truncated")
        }
        LineOp::IndentLeft => shift_indent(doc, line, repeat.max(1), false),
        LineOp::IndentRight => shift_indent(doc, line, if repeat == 0 { 1 } else { repeat }, true),
        LineOp::Split => {
            let col = if repeat == 0 { doc.cursor().1 } else { repeat };
            doc.split_line(line, col)?;
            Ok("Split".to_string())
        }
        LineOp::Flow | LineOp::Margin => {
            flow_from(doc, line, if repeat == 0 { 72 } else { repeat })
        }
        LineOp::Note => {
            let count = repeat.max(1);
            for _ in 0..count {
                doc.insert_profile(line, LineFlags::NOTE, "")?;
            }
            Ok(format!("Inserted {count} note{}", plural(count)))
        }
        LineOp::MakeNote => tag_count(doc, line, repeat.max(1), Some(true), None, "Note"),
        LineOp::MakeData => tag_count(doc, line, repeat.max(1), Some(false), None, "Data"),
        LineOp::User => tag_count(doc, line, repeat.max(1), None, Some(true), "User line"),
        LineOp::Revert => tag_count(doc, line, repeat.max(1), None, Some(false), "Reverted"),
        LineOp::ToggleX => {
            let end = doc.span_end(line, repeat.max(1))?;
            let n = doc.toggle_excluded(line, end)?;
            Ok(format!("Toggled {n} line{}", plural(n)))
        }
        LineOp::ToggleUser => tag_count(doc, line, repeat.max(1), None, None, "Toggled user lines"),
        LineOp::Cols => insert_named(doc, line, LineFlags::COLS, &ruler()),
        LineOp::Tabs => insert_named(doc, line, LineFlags::TABS, &tab_ruler()),
        LineOp::Bnds => insert_named(doc, line, LineFlags::BOUNDS, &bounds_ruler(doc)),
        LineOp::Mask => insert_named(doc, line, LineFlags::MASK, ""),
        LineOp::Word => insert_named(doc, line, LineFlags::WORD, " "),
        LineOp::Mark => insert_named(doc, line, LineFlags::MARK, &ruler()),
        LineOp::Text => {
            doc.set_cursor(line, 1)?;
            Ok("Cursor moved".to_string())
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineOp {
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
    Split,
    Flow,
    Margin,
    Note,
    MakeNote,
    MakeData,
    User,
    Revert,
    ToggleX,
    ToggleUser,
    Cols,
    Tabs,
    Bnds,
    Mask,
    Word,
    Mark,
    Text,
}

impl LineOp {
    pub(crate) fn pair(self) -> PairKind {
        match self {
            LineOp::Join => PairKind::Join,
            LineOp::Glue => PairKind::Glue,
            LineOp::TextJoin => PairKind::TextJoin,
            LineOp::TextGlue => PairKind::TextGlue,
            LineOp::Sentence => PairKind::Sentence,
            LineOp::Title => PairKind::Title,
            LineOp::Trim => PairKind::Trim,
            LineOp::Pad => PairKind::Pad,
            LineOp::Trunc => PairKind::Trunc,
            LineOp::IndentLeft => PairKind::IndentLeft,
            LineOp::IndentRight => PairKind::IndentRight,
            LineOp::Flow => PairKind::Flow,
            LineOp::Margin => PairKind::Margin,
            LineOp::MakeNote => PairKind::MakeNote,
            LineOp::MakeData => PairKind::MakeData,
            LineOp::User => PairKind::User,
            LineOp::Revert => PairKind::Revert,
            LineOp::ToggleX => PairKind::ToggleX,
            LineOp::ToggleUser => PairKind::ToggleUser,
            _ => PairKind::Join,
        }
    }

    pub(crate) fn pairs(self) -> bool {
        !matches!(
            self,
            LineOp::Split
                | LineOp::Note
                | LineOp::Cols
                | LineOp::Tabs
                | LineOp::Bnds
                | LineOp::Mask
                | LineOp::Word
                | LineOp::Mark
                | LineOp::Text
        )
    }
}

pub(crate) fn apply_dest(
    doc: &mut Document,
    start: usize,
    end: usize,
    dest_start: usize,
    dest_end: usize,
    kind: DestKind,
    delete_source: bool,
) -> EditResult<LineShift> {
    match kind {
        DestKind::Overlay => {
            doc.overlay_lines(start, end, dest_start, dest_end, false, delete_source)
        }
        DestKind::OverlayReplace => {
            doc.overlay_lines(start, end, dest_start, dest_end, true, delete_source)
        }
        DestKind::Here => {
            if dest_start == dest_end {
                doc.copy_move(start, end, dest_start, true, 1, delete_source)
            } else {
                replace_with_source(doc, start, end, dest_start, dest_end, delete_source)
            }
        }
        DestKind::AfterEach => {
            doc.copy_each(start, end, dest_start, dest_end, false, delete_source)
        }
        DestKind::BeforeEach => {
            doc.copy_each(start, end, dest_start, dest_end, true, delete_source)
        }
        DestKind::Swap => doc.swap_ranges(start, end, dest_start, dest_end),
    }
}

fn replace_with_source(
    doc: &mut Document,
    start: usize,
    end: usize,
    dest_start: usize,
    dest_end: usize,
    delete_source: bool,
) -> EditResult<LineShift> {
    let texts: Vec<String> = (start..=end)
        .map(|line| doc.line_text(line).unwrap_or("").to_string())
        .collect();
    let dest_len = dest_end - dest_start + 1;
    let src_len = texts.len();
    doc.delete_range(dest_start, dest_end)?;
    doc.insert_text_lines(dest_start.saturating_sub(1).max(1), &texts)?;
    if delete_source {
        let src = if start > dest_end {
            start - dest_len + src_len
        } else {
            start
        };
        doc.delete_range(src, src + src_len - 1)?;
    }
    Ok(LineShift::identity())
}

fn block_pair(doc: &mut Document, line: usize, kind: LineOp, count: usize) -> EditResult<String> {
    if !kind.pairs() {
        return Err(EditError::new("That line command has no block form"));
    }
    let pair = kind.pair();
    match doc.pending() {
        crate::document::PendingBlock::Pair {
            line: other,
            kind: pending,
            count: prev,
        } if pending == pair => {
            doc.set_pending(crate::document::PendingBlock::None);
            if other == line {
                return Ok(format!("{} cancelled", pair_name(pair)));
            }
            let (start, end) = ordered(other, line);
            apply_block(doc, start, end, kind, merge_count(prev, count))
        }
        crate::document::PendingBlock::None => {
            doc.set_pending(crate::document::PendingBlock::Pair {
                line,
                kind: pair,
                count,
            });
            Ok(format!("{} pending", pair_name(pair)))
        }
        other => Err(EditError::new(format!(
            "A {} block is still open",
            pending_label(other)
        ))),
    }
}

fn apply_block(
    doc: &mut Document,
    start: usize,
    end: usize,
    kind: LineOp,
    count: usize,
) -> EditResult<String> {
    match kind {
        LineOp::Join => {
            doc.join_range(start, end, " ", false)?;
            Ok("Joined".to_string())
        }
        LineOp::Glue => {
            doc.join_range(start, end, "", false)?;
            Ok("Glued".to_string())
        }
        LineOp::TextJoin => {
            doc.join_range(start, end, " ", true)?;
            Ok("Joined".to_string())
        }
        LineOp::TextGlue => {
            doc.join_range(start, end, "", true)?;
            Ok("Glued".to_string())
        }
        LineOp::Sentence => map_range(doc, start, end, CaseKind::Sentence),
        LineOp::Title => map_range(doc, start, end, CaseKind::Title),
        LineOp::Trim => map_range_text(
            doc,
            start,
            end,
            |text| text.trim_end().to_string(),
            "Trimmed",
        ),
        LineOp::Pad => {
            let width = if count == 0 { 80 } else { count };
            map_range_text(doc, start, end, |text| pad_line(text, width), "Padded")
        }
        LineOp::Trunc => {
            let width = if count == 0 { 80 } else { count };
            map_range_text(doc, start, end, |text| trunc_line(text, width), "Truncated")
        }
        LineOp::IndentLeft => {
            let cols = if count == 0 { 1 } else { count };
            doc.map_span(start, end, |text| indent(text, cols, false))?;
            Ok(format!("Shifted {cols}"))
        }
        LineOp::IndentRight => {
            let cols = if count == 0 { 1 } else { count };
            doc.map_span(start, end, |text| indent(text, cols, true))?;
            Ok(format!("Shifted {cols}"))
        }
        LineOp::Flow | LineOp::Margin => {
            let width = if count == 0 { 72 } else { count };
            reflow(doc, start, end, width)
        }
        LineOp::MakeNote => {
            doc.tag_lines(start, end, Some(true), None)?;
            Ok("Note".to_string())
        }
        LineOp::MakeData => {
            doc.tag_lines(start, end, Some(false), None)?;
            Ok("Data".to_string())
        }
        LineOp::User => {
            doc.tag_lines(start, end, None, Some(true))?;
            Ok("User line".to_string())
        }
        LineOp::Revert => {
            doc.tag_lines(start, end, None, Some(false))?;
            Ok("Reverted".to_string())
        }
        LineOp::ToggleX => {
            let n = doc.toggle_excluded(start, end)?;
            Ok(format!("Toggled {n} line{}", plural(n)))
        }
        LineOp::ToggleUser => {
            doc.tag_lines(start, end, None, None)?;
            Ok("Toggled user lines".to_string())
        }
        _ => Err(EditError::new("That line command has no block form")),
    }
}

fn pending_label(pending: crate::document::PendingBlock) -> String {
    match pending {
        crate::document::PendingBlock::Pair { kind, .. } => pair_name(kind).to_string(),
        crate::document::PendingBlock::Delete(_) => "DD".to_string(),
        crate::document::PendingBlock::Exclude(_) => "XX".to_string(),
        _ => "block".to_string(),
    }
}

fn join_lines(
    doc: &mut Document,
    line: usize,
    extra: usize,
    sep: &str,
    squeeze: bool,
) -> EditResult<String> {
    let end = doc.span_end(line, extra + 1)?;
    doc.join_range(line, end, sep, squeeze)?;
    Ok(if sep.is_empty() && !squeeze {
        "Glued".to_string()
    } else {
        "Joined".to_string()
    })
}

fn map_count(doc: &mut Document, line: usize, count: usize, kind: CaseKind) -> EditResult<String> {
    let end = doc.span_end(line, count)?;
    map_range(doc, line, end, kind)
}

fn map_count_text(
    doc: &mut Document,
    line: usize,
    count: usize,
    edit: impl Fn(&str) -> String,
    message: &str,
) -> EditResult<String> {
    let end = doc.span_end(line, count)?;
    map_range_text(doc, line, end, edit, message)
}

fn map_range(doc: &mut Document, start: usize, end: usize, kind: CaseKind) -> EditResult<String> {
    doc.map_span(start, end, |text| case_text(text, kind))?;
    Ok(match kind {
        CaseKind::Upper => "Uppercased".to_string(),
        CaseKind::Lower => "Lowercased".to_string(),
        CaseKind::Sentence => "Sentence case".to_string(),
        CaseKind::Title => "Title case".to_string(),
    })
}

fn map_range_text(
    doc: &mut Document,
    start: usize,
    end: usize,
    edit: impl Fn(&str) -> String,
    message: &str,
) -> EditResult<String> {
    doc.map_span(start, end, edit)?;
    Ok(message.to_string())
}

fn shift_indent(doc: &mut Document, line: usize, cols: usize, right: bool) -> EditResult<String> {
    doc.map_span(line, line, |text| indent(text, cols, right))?;
    Ok(format!("Shifted {cols}"))
}

fn tag_count(
    doc: &mut Document,
    line: usize,
    count: usize,
    note: Option<bool>,
    user: Option<bool>,
    message: &str,
) -> EditResult<String> {
    let end = doc.span_end(line, count)?;
    doc.tag_lines(line, end, note, user)?;
    Ok(message.to_string())
}

fn flow_from(doc: &mut Document, line: usize, width: usize) -> EditResult<String> {
    let mut end = line;
    while end + 1 < doc.last_line() {
        let next = doc.line_text(end + 1).unwrap_or("");
        if next.trim().is_empty() {
            break;
        }
        if !doc.line(end + 1)?.is_data() && !doc.line(end + 1)?.is_note() {
            break;
        }
        end += 1;
    }
    reflow(doc, line, end, width)
}

fn reflow(doc: &mut Document, start: usize, end: usize, width: usize) -> EditResult<String> {
    let mut words = Vec::new();
    for line in start..=end {
        if doc.line(line)?.is_data() || doc.line(line)?.is_note() {
            words.extend(doc.line_text(line)?.split_whitespace().map(str::to_string));
        }
    }
    let rows = wrap_words(&words, width.max(1));
    doc.replace_span_with(start, end, &rows)?;
    Ok(format!("Flowed to {width}"))
}

fn insert_named(doc: &mut Document, line: usize, flag: u32, text: &str) -> EditResult<String> {
    doc.insert_profile(line, flag, text)?;
    Ok("Profile line inserted".to_string())
}

fn clip_range(doc: &Document, rest: &str) -> EditResult<(usize, usize)> {
    let rest = rest.trim();
    if rest.is_empty() {
        let line = data_line(doc, doc.cursor().0)?;
        return Ok((line, line));
    }
    if rest.eq_ignore_ascii_case("ALL") {
        if doc.last_real() == 0 {
            return Err(EditError::new("No lines to copy"));
        }
        return Ok((2, doc.last_line() - 1));
    }
    let count: usize = rest
        .parse()
        .map_err(|_| EditError::new("CUT needs a count or ALL"))?;
    let start = data_line(doc, doc.cursor().0)?;
    Ok((start, doc.span_end(start, count.max(1))?))
}

fn collect_text(doc: &Document, start: usize, end: usize) -> EditResult<Vec<String>> {
    let mut lines = Vec::new();
    for line in start..=end {
        let row = doc.line(line)?;
        if row.is_data() || row.is_note() {
            lines.push(row.text().to_string());
        }
    }
    if lines.is_empty() {
        return Err(EditError::new("No lines to copy"));
    }
    Ok(lines)
}

fn lines_without(doc: &Document, rest: &str) -> EditResult<Vec<usize>> {
    let (text, case_sensitive) = search_text(rest)?;
    let mut lines = Vec::new();
    for line in 1..doc.last_line() {
        let row = doc.line(line)?;
        if (row.is_data() || row.is_note())
            && !row.is_invisible()
            && !contains(row.text(), &text, case_sensitive)
        {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return Err(EditError::new("No lines to change"));
    }
    Ok(lines)
}

fn search_text(rest: &str) -> EditResult<(String, bool)> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(EditError::new("Missing string"));
    }
    let (word, _) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if let Some(text) = word
        .strip_prefix("C'")
        .and_then(|body| body.strip_suffix('\''))
    {
        return Ok((text.to_string(), true));
    }
    if let Some(text) = word
        .strip_prefix('\'')
        .and_then(|body| body.strip_suffix('\''))
    {
        return Ok((text.to_string(), false));
    }
    Ok((word.to_string(), false))
}

fn data_line(doc: &Document, line: usize) -> EditResult<usize> {
    let row = doc.line(line)?;
    if row.is_data() || row.is_note() {
        Ok(line)
    } else if line + 1 < doc.last_line()
        && (doc.line(line + 1)?.is_data() || doc.line(line + 1)?.is_note())
    {
        Ok(line + 1)
    } else {
        Err(EditError::new("Cursor is not on a data line"))
    }
}

fn optional_count(rest: &str, name: &str) -> EditResult<Option<usize>> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(None);
    }
    rest.parse::<usize>()
        .map(Some)
        .map_err(|_| EditError::new(format!("{name} count is invalid")))
}

fn need_path(rest: &str, name: &str) -> EditResult<String> {
    let path = rest.trim();
    if path.is_empty() {
        return Err(EditError::new(format!("{name} needs a file name")));
    }
    Ok(path.to_string())
}

fn write_data(doc: &Document, path: &str) -> EditResult<()> {
    fs::write(path, doc.to_bytes()?)
        .map_err(|err| EditError::new(format!("Cannot write {path}: {err}")))
}

fn contains(text: &str, needle: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        text.contains(needle)
    } else {
        text.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }
}

fn ordered(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn merge_count(left: usize, right: usize) -> usize {
    match (left, right) {
        (0, value) | (value, 0) => value,
        _ => left,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn case_text(text: &str, kind: CaseKind) -> String {
    match kind {
        CaseKind::Upper => text.to_uppercase(),
        CaseKind::Lower => text.to_lowercase(),
        CaseKind::Sentence => sentence_case(text),
        CaseKind::Title => title_case(text),
    }
}

fn sentence_case(text: &str) -> String {
    let mut out = String::new();
    let mut cap = true;
    for ch in text.chars() {
        if ch.is_ascii_alphabetic() {
            if cap {
                out.extend(ch.to_uppercase());
            } else {
                out.extend(ch.to_lowercase());
            }
            cap = false;
        } else {
            if matches!(ch, '.' | '!' | '?') {
                cap = true;
            }
            out.push(ch);
        }
    }
    out
}

fn title_case(text: &str) -> String {
    let mut out = String::new();
    let mut cap = true;
    for ch in text.chars() {
        if ch.is_ascii_alphabetic() {
            if cap {
                out.extend(ch.to_uppercase());
            } else {
                out.extend(ch.to_lowercase());
            }
            cap = false;
        } else {
            cap = ch.is_whitespace();
            out.push(ch);
        }
    }
    out
}

fn pad_line(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

fn trunc_line(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

fn indent(text: &str, cols: usize, right: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    if right {
        let mut out = vec![' '; cols];
        out.extend(chars);
        return out.into_iter().collect();
    }
    let have = chars.iter().take_while(|ch| ch.is_whitespace()).count();
    let drop = cols.min(have);
    chars[drop..].iter().collect()
}

fn squeeze(text: &str) -> String {
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

fn wrap_words(words: &[String], width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    for word in words {
        if current.is_empty() {
            current = word.clone();
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            rows.push(std::mem::take(&mut current));
            current = word.clone();
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn ruler() -> String {
    let mut out = String::new();
    for col in 1..=80 {
        if col % 10 == 0 {
            out.push(char::from_digit((col / 10) as u32, 10).unwrap_or('+'));
        } else if col % 5 == 0 {
            out.push('+');
        } else {
            out.push('-');
        }
    }
    out
}

fn tab_ruler() -> String {
    let mut out = String::new();
    for col in 1..=80 {
        if col % 8 == 1 {
            out.push('*');
        } else {
            out.push('.');
        }
    }
    out
}

fn bounds_ruler(doc: &Document) -> String {
    let left = doc.profile().bounds_left.max(1);
    let right = if doc.profile().bounds_right == 0 {
        80
    } else {
        doc.profile().bounds_right
    };
    let width = right.max(left).max(80);
    let mut chars = vec![' '; width];
    if left - 1 < chars.len() {
        chars[left - 1] = '<';
    }
    if right - 1 < chars.len() {
        chars[right - 1] = '>';
    }
    chars.into_iter().collect()
}
