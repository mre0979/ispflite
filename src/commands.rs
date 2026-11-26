// SPDX-License-Identifier: GPL-3.0-or-later
//! Primary commands and line commands.
//!
//! Parse a command, change the document, and return the status message.
//! Line commands use a leading `:`. `:5 D` is a `D` on sequence line 5.
//! `L 5` is LOCATE, as in SPFLite. The window submits every prefix on Enter
//! together, so `C` and `A` on that Enter are one copy.

use crate::document::{Document, FindSpan, LineShift, PendingBlock};
use crate::error::{EditError, EditResult};
use crate::io::Encoding;
use crate::session::Session;

/// Run one or more commands separated by `;`. Stops at the first error.
/// Commands before the error stay applied.
pub fn execute(session: &mut Session, input: &str) -> EditResult<String> {
    let statements = split_statements(input);
    if statements.is_empty() {
        return Ok(String::new());
    }
    let mut messages = Vec::new();
    for statement in statements {
        messages.push(execute_one(session, &statement)?);
    }
    Ok(messages.join(" "))
}

fn execute_one(session: &mut Session, input: &str) -> EditResult<String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(String::new());
    }
    session.current_mut().doc.clear_find_highlights();
    if !command_is_rfind(input) {
        session.current_mut().doc.clear_rfind_edge();
    }
    if let Some(rest) = input.strip_prefix(':') {
        let doc = &mut session.current_mut().doc;
        return line_at(doc, rest);
    }
    if input.starts_with('.') {
        return locate(&mut session.current_mut().doc, input);
    }
    let (verb, rest) = split_verb(input);
    let verb = verb.to_ascii_uppercase();
    match verb.as_str() {
        "FIND" | "F" => find(&mut session.current_mut().doc, rest),
        "CHANGE" | "CHG" | "C" => change(&mut session.current_mut().doc, rest),
        "LOCATE" | "LOC" | "L" => locate(&mut session.current_mut().doc, rest),
        "TEXT" => set_text(&mut session.current_mut().doc, rest),
        "LABEL" => set_label_cmd(&mut session.current_mut().doc, rest),
        "TAG" => set_tag_cmd(&mut session.current_mut().doc, rest),
        "BOUNDS" | "BNDS" => bounds(&mut session.current_mut().doc, rest),
        "RESET" | "RES" => reset(&mut session.current_mut().doc, rest),
        "UNDO" => {
            session.current_mut().doc.undo()?;
            Ok("Undone".to_string())
        }
        "REDO" => {
            session.current_mut().doc.redo()?;
            Ok("Redone".to_string())
        }
        "SAVE" => save(session, rest),
        "OPEN" | "EDIT" => open(session, rest),
        "ENCODING" => encoding(session, rest),
        "TABS" => Ok(session.tab_list()),
        "TAB" => switch_tab(session, rest),
        "HELP" | "?" => Ok(help_text()),
        "DELETE" | "DEL" => delete_matches(&mut session.current_mut().doc, rest),
        "EXCLUDE" | "EX" | "X" => exclude_matches(&mut session.current_mut().doc, rest),
        "SORT" => sort_cmd(&mut session.current_mut().doc, rest),
        "TOP" => scroll_edge(&mut session.current_mut().doc, true, rest),
        "BOTTOM" | "BOT" => scroll_edge(&mut session.current_mut().doc, false, rest),
        "BOOKMARK" | "BM" => bookmark(&mut session.current_mut().doc, rest),
        "RFIND" | "RF" => crate::more::rfind(&mut session.current_mut().doc, rest),
        "RCHANGE" | "RCHG" | "RC" => crate::more::rchange(&mut session.current_mut().doc),
        "RLOCATE" | "RLOC" | "RL" => crate::more::rlocate(&mut session.current_mut().doc),
        "NFIND" | "NF" => crate::more::nfind(&mut session.current_mut().doc, rest),
        "NEXCLUDE" | "NX" => crate::more::nexclude(&mut session.current_mut().doc, rest),
        "NDELETE" | "NDEL" | "ND" => crate::more::ndelete(&mut session.current_mut().doc, rest),
        "FLIP" => crate::more::flip(&mut session.current_mut().doc, rest),
        "SHOW" => crate::more::show(&mut session.current_mut().doc, rest),
        "HIDE" => crate::more::hide(&mut session.current_mut().doc, rest),
        "CUT" => crate::more::cut(session, rest),
        "PASTE" => crate::more::paste(session, rest),
        "CLIP" => crate::more::clip(session, rest),
        "COPY" => crate::more::copy_file(&mut session.current_mut().doc, rest),
        "CREATE" => crate::more::create_file(&session.current().doc, rest),
        "REPLACE" => crate::more::replace_file(&session.current().doc, rest),
        "SAVEAS" => save(session, rest),
        "CANCEL" | "CAN" => crate::more::cancel(session, rest),
        "EXIT" => crate::more::exit_cmd(session, rest),
        "RENAME" | "REN" => crate::more::rename(session, rest),
        "JOIN" => crate::more::join_at_cursor(&mut session.current_mut().doc, rest),
        "SPLIT" => crate::more::split_at_cursor(&mut session.current_mut().doc, rest),
        "APPEND" => crate::more::append_text(&mut session.current_mut().doc, rest),
        "PREPEND" => crate::more::prepend_text(&mut session.current_mut().doc, rest),
        "ALIGN" => crate::more::align(&mut session.current_mut().doc, rest),
        "COMPRESS" => crate::more::compress(&mut session.current_mut().doc, rest),
        "UC" => crate::more::case_all(
            &mut session.current_mut().doc,
            rest,
            crate::more::CaseKind::Upper,
        ),
        "LC" => crate::more::case_all(
            &mut session.current_mut().doc,
            rest,
            crate::more::CaseKind::Lower,
        ),
        "SC" => crate::more::case_all(
            &mut session.current_mut().doc,
            rest,
            crate::more::CaseKind::Sentence,
        ),
        "TC" => crate::more::case_all(
            &mut session.current_mut().doc,
            rest,
            crate::more::CaseKind::Title,
        ),
        "LINE" => crate::more::line_number(&mut session.current_mut().doc, rest),
        "HEX" => crate::more::hex(&mut session.current_mut().doc, rest),
        "SWAPBAR" => swapbar(session, rest),
        "SWAP" => swap(session, rest),
        "START" => start_screen(session, rest),
        "COLS" | "COL" => crate::more::cols(&mut session.current_mut().doc, rest),
        "RENUM" | "RENUMBER" => crate::more::renum(&mut session.current_mut().doc, rest),
        "UP" => crate::more::move_cursor(&mut session.current_mut().doc, rest, false, true),
        "DOWN" => crate::more::move_cursor(&mut session.current_mut().doc, rest, true, true),
        "LEFT" => crate::more::move_cursor(&mut session.current_mut().doc, rest, false, false),
        "RIGHT" => crate::more::move_cursor(&mut session.current_mut().doc, rest, true, false),
        _ => Err(EditError::new(format!("Unknown command '{verb}'"))),
    }
}

fn command_is_rfind(input: &str) -> bool {
    let (verb, _) = split_verb(input);
    matches!(verb.to_ascii_uppercase().as_str(), "RFIND" | "RF")
}

fn find(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (needle, which, qualifiers) = parse_search(doc, rest)?;
    doc.remember_find(&needle.text, needle.case_sensitive, &qualifiers);
    doc.clear_rfind_edge();
    if which == Which::All {
        return find_all(doc, &needle);
    }
    let hit = seek(doc, &needle, which, true, doc.screen_find_line())?;
    show_find(doc, &needle, hit)
}

fn show_find(doc: &mut Document, needle: &Needle, hit: Hit) -> EditResult<String> {
    let hidden = doc.line(hit.line)?.is_invisible();
    let line = if hidden {
        doc.pop_out(hit.line)?
    } else {
        hit.line
    };
    doc.mark_find(line);
    let marks = collect_finds(doc, needle);
    doc.set_find_highlights(if marks.is_empty() {
        vec![FindSpan {
            line,
            col: hit.col,
            len: hit.len,
        }]
    } else {
        marks
    });
    doc.place_found_cursor(line, hit.col);
    let seq = doc.line(line)?.seq;
    Ok(format!(
        "Found '{}' on line {seq} column {}",
        needle.text, hit.col
    ))
}

fn find_all(doc: &mut Document, needle: &Needle) -> EditResult<String> {
    let marks = collect_finds(doc, needle);
    if marks.is_empty() {
        return Err(not_found(needle));
    }
    // Every excluded line that holds a hit is shown. Later lines are opened
    // first so the earlier indexes stay put.
    let mut hidden = Vec::new();
    let mut previous = 0usize;
    for mark in &marks {
        if mark.line == previous {
            continue;
        }
        previous = mark.line;
        if doc.line(mark.line)?.is_invisible() {
            hidden.push(mark.line);
        }
    }
    if !hidden.is_empty() {
        doc.push_undo(false);
        for line in hidden.into_iter().rev() {
            doc.pop_out_silent(line)?;
        }
    }
    let marks = collect_finds(doc, needle);
    let count = marks.len();
    let mut lines = 0usize;
    let mut previous = 0usize;
    for mark in &marks {
        if mark.line != previous {
            lines += 1;
            previous = mark.line;
            doc.mark_find(mark.line);
        }
    }
    let first = marks[0].col;
    let line = marks[0].line;
    doc.set_find_highlights(marks);
    doc.place_found_cursor(line, first);
    Ok(format!(
        "{count} occurrence{} of '{}' on {lines} line{}",
        plural(count),
        needle.text,
        plural(lines)
    ))
}

fn collect_finds(doc: &Document, needle: &Needle) -> Vec<FindSpan> {
    let mut marks = Vec::new();
    for line in 1..doc.last_line() {
        for col in match_columns(doc, line, needle) {
            marks.push(FindSpan {
                line,
                col,
                len: needle.len,
            });
        }
    }
    marks
}

pub(crate) fn find_again(
    doc: &mut Document,
    text: &str,
    case_sensitive: bool,
    qualifiers: &str,
    upward: bool,
) -> EditResult<String> {
    let quoted = quote_operand(text, case_sensitive);
    let kept = without_which(qualifiers);
    let rest = if upward {
        format!("{quoted} PREV {kept}")
    } else {
        format!("{quoted} {kept}")
    };
    let (needle, which, _) = parse_search(doc, &rest)?;
    doc.remember_find(&needle.text, needle.case_sensitive, &kept);
    let (line, col) = doc.cursor();
    if doc.take_rfind_edge(upward, line, col) {
        let hit = if upward {
            find_backward(doc, &needle, doc.last_line(), usize::MAX, None)
        } else {
            find_forward(doc, &needle, 1, 1, None)
        };
        return match hit {
            Some(hit) => show_find(doc, &needle, hit),
            None => Err(not_found(&needle)),
        };
    }
    doc.clear_rfind_edge();
    match seek(doc, &needle, which, true, None) {
        Ok(hit) => show_find(doc, &needle, hit),
        Err(error) => {
            if find_forward(doc, &needle, 1, 1, None).is_none() {
                return Err(error);
            }
            doc.arm_rfind_edge(upward, line, col);
            if doc.cursor_on_find() {
                doc.set_find_highlights(vec![FindSpan {
                    line,
                    col,
                    len: needle.len,
                }]);
            }
            Ok(if upward {
                "Top of data reached".to_string()
            } else {
                "Bottom of data reached".to_string()
            })
        }
    }
}

pub(crate) fn change_again(
    doc: &mut Document,
    from: &str,
    case_sensitive: bool,
    to: &str,
    qualifiers: &str,
) -> EditResult<String> {
    // Repeat from the cursor. A new CHANGE starts at the top of the screen.
    let origin = doc.screen_find_line();
    doc.set_screen_find_line(None);
    let result = change(
        doc,
        &format!(
            "{} {} {}",
            quote_operand(from, case_sensitive),
            quote_operand(to, false),
            without_which(qualifiers)
        ),
    );
    doc.set_screen_find_line(origin);
    result
}

pub(crate) fn locate_again(doc: &mut Document, operand: &str) -> EditResult<String> {
    locate(doc, operand)
}

fn quote_operand(text: &str, case_sensitive: bool) -> String {
    let escaped = text.replace('\'', "''");
    if case_sensitive {
        format!("C'{escaped}'")
    } else {
        format!("'{escaped}'")
    }
}

fn change(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (old, new_text, which, qualifiers) = parse_change(doc, rest)?;
    doc.remember_change(&old.text, old.case_sensitive, &new_text, &qualifiers);
    if which == Which::All {
        return change_all(doc, &old, &new_text);
    }
    let hit = seek(doc, &old, which, false, doc.screen_find_line())?;
    let hidden = doc.line(hit.line)?.is_invisible();
    doc.push_undo(true);
    let line = if hidden {
        doc.pop_out_silent(hit.line)?
    } else {
        hit.line
    };
    doc.replace_columns(line, hit.col, hit.len, &new_text)?;
    let after = hit.col + new_text.chars().count();
    doc.set_cursor(line, after.max(1))?;
    let mut marks = collect_finds(doc, &old);
    let new_len = new_text.chars().count();
    if new_len > 0 {
        marks.push(FindSpan {
            line,
            col: hit.col,
            len: new_len,
        });
    }
    doc.set_find_highlights(marks);
    doc.mark_find(line);
    Ok(format!("Changed '{}' to '{}'", old.text, new_text))
}

fn change_all(doc: &mut Document, old: &Needle, new_text: &str) -> EditResult<String> {
    let last = doc.last_line();
    let mut plan: Vec<(usize, Vec<usize>)> = Vec::new();
    for line in 1..=last {
        let hits = match_columns(doc, line, old);
        if !hits.is_empty() {
            plan.push((line, hits));
        }
    }
    if plan.is_empty() {
        return Err(not_found(old));
    }
    doc.push_undo(true);
    let mut count = 0usize;
    let new_len = new_text.chars().count();
    let mut marks = Vec::new();
    for (line, hits) in plan.into_iter().rev() {
        for col in hits.into_iter().rev() {
            doc.replace_columns(line, col, old.len, new_text)?;
            if new_len > 0 {
                marks.push(FindSpan {
                    line,
                    col,
                    len: new_len,
                });
            }
            count += 1;
        }
    }
    doc.set_find_highlights(marks);
    Ok(format!(
        "{count} occurrence{} of '{}' changed",
        plural(count),
        old.text
    ))
}

fn locate(doc: &mut Document, rest: &str) -> EditResult<String> {
    let reference = rest.trim();
    if reference.is_empty() {
        return Err(EditError::new("LOCATE needs a line"));
    }
    doc.remember_locate(reference);
    let mut line = doc.resolve_line(reference)?;
    if doc.line(line)?.is_invisible() {
        line = doc.pop_out(line)?;
    }
    doc.set_cursor(line, 1)?;
    Ok(format!("Cursor on line {}", doc.line(line)?.seq))
}

fn set_text(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (reference, text) = split_token(rest)?;
    let line = doc.resolve_line(reference)?;
    let text = unwrap_quotes(text);
    doc.set_line_text(line, &text)?;
    doc.set_cursor(line, 1)?;
    Ok("Line updated".to_string())
}

fn set_label_cmd(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (reference, name) = split_token_optional(rest)?;
    let line = doc.resolve_line(reference)?;
    if name.is_empty() {
        doc.set_label(line, None)?;
        Ok("Label cleared".to_string())
    } else {
        doc.set_label(line, Some(name))?;
        Ok(format!("Label {name} set"))
    }
}

fn set_tag_cmd(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (reference, name) = split_token_optional(rest)?;
    let line = doc.resolve_line(reference)?;
    if name.is_empty() {
        doc.set_tag(line, None)?;
        Ok("Tag cleared".to_string())
    } else {
        doc.set_tag(line, Some(name))?;
        Ok(format!("Tag {name} set"))
    }
}

fn bounds(doc: &mut Document, rest: &str) -> EditResult<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        doc.set_bounds(1, 0)?;
        return Ok("Bounds are the whole line".to_string());
    }
    let mut parts = rest.split_whitespace();
    let left: usize = parts
        .next()
        .unwrap()
        .parse()
        .map_err(|_| EditError::new("Left bound is not a number"))?;
    let right = match parts.next() {
        Some(token) => token
            .parse()
            .map_err(|_| EditError::new("Right bound is not a number"))?,
        None => 0,
    };
    if parts.next().is_some() {
        return Err(EditError::new("BOUNDS takes one or two columns"));
    }
    doc.set_bounds(left, right)?;
    if right == 0 {
        Ok(format!("Bounds are {left} through the end of the line"))
    } else {
        Ok(format!("Bounds are {left} {right}"))
    }
}

/// ISPF RESET. With no operands, every kind is cleared except labels.
fn reset(doc: &mut Document, rest: &str) -> EditResult<String> {
    let mut change = false;
    let mut command = false;
    let mut excluded = false;
    let mut find = false;
    let mut label = false;
    let mut special = false;
    let mut all = false;
    let mut named = false;
    let mut labels = Vec::new();
    for token in rest.split_whitespace() {
        match token.to_ascii_uppercase().as_str() {
            "CHANGE" | "CHG" | "C" => {
                change = true;
                named = true;
            }
            "COMMAND" | "CMD" | "COM" => {
                command = true;
                named = true;
            }
            "ERROR" | "ERR" => named = true,
            "EXCLUDED" | "EXCLUDE" | "EXC" | "EX" | "X" => {
                excluded = true;
                named = true;
            }
            "FIND" | "F" => {
                find = true;
                named = true;
            }
            "HIDE" => named = true,
            "LABEL" | "LABELS" | "LAB" => {
                label = true;
                named = true;
            }
            "SOURCE" => named = true,
            "SPECIAL" | "SPE" => {
                special = true;
                named = true;
            }
            "ALL" => {
                all = true;
                named = true;
            }
            _ if token.starts_with('.') => labels.push(token),
            other => return Err(EditError::new(format!("Unknown keyword '{other}'"))),
        }
    }
    if labels.len() != 2 && !labels.is_empty() {
        return Err(EditError::new("RESET range needs two labels"));
    }
    if !named {
        change = true;
        command = true;
        excluded = true;
        find = true;
        special = true;
    }
    if all {
        change = true;
        command = true;
        label = true;
    }
    let range = if labels.len() == 2 {
        let start = doc.resolve_line(labels[0])?;
        let end = doc.resolve_line(labels[1])?;
        Some((start.min(end), start.max(end)))
    } else {
        None
    };
    let in_span = |index: usize| match range {
        Some((start, end)) => index >= start && index <= end,
        None => true,
    };
    let delete_special = special
        && doc
            .lines
            .iter()
            .enumerate()
            .any(|(index, line)| in_span(index) && temporary_line(line));
    let show = excluded
        && doc
            .lines
            .iter()
            .enumerate()
            .any(|(index, line)| in_span(index) && line.is_invisible());
    let clear_change = change
        && doc.lines.iter().enumerate().any(|(index, line)| {
            in_span(index) && line.flags.contains(crate::line::LineFlags::EQ_CHANGE)
        });
    let clear_labels = label
        && doc
            .lines
            .iter()
            .enumerate()
            .any(|(index, line)| in_span(index) && line.label.is_some());
    let clear_pending = command
        && doc.pending() != PendingBlock::None
        && match range {
            None => true,
            Some((start, end)) => (start..=end).any(|line| doc.pending().covers(line)),
        };
    let clear_find = find && (1..doc.lines.len()).any(|line| !doc.find_spans(line).is_empty());
    let clear_cols = special && range.is_none() && doc.cols_mode();
    if delete_special
        || show
        || clear_change
        || clear_labels
        || clear_pending
        || clear_find
        || clear_cols
    {
        doc.push_undo(delete_special);
        if clear_change || clear_labels || show {
            for index in 0..doc.lines.len() {
                if !in_span(index) {
                    continue;
                }
                if clear_change {
                    doc.lines[index]
                        .flags
                        .remove(crate::line::LineFlags::EQ_CHANGE);
                }
                if clear_labels {
                    doc.lines[index].label = None;
                }
                if show && doc.lines[index].is_invisible() {
                    doc.lines[index]
                        .flags
                        .remove(crate::line::LineFlags::INVISIBLE);
                    if !doc.lines[index].is_xclude() {
                        doc.lines[index].work1 = 0;
                    }
                }
            }
        }
        if clear_pending {
            doc.set_pending(PendingBlock::None);
        }
        if clear_find {
            doc.clear_find_highlights();
        }
        if clear_cols {
            doc.set_cols(false);
        }
        if delete_special {
            let doomed: Vec<usize> = doc
                .lines
                .iter()
                .enumerate()
                .filter(|(index, line)| in_span(*index) && temporary_line(line))
                .map(|(index, _)| index)
                .collect();
            for index in doomed.into_iter().rev() {
                doc.lines.remove(index);
            }
        }
        if show || delete_special {
            doc.rebuild_excludes();
            doc.renumber();
            doc.clamp_cursor();
        }
    }
    Ok("Reset".to_string())
}

fn temporary_line(line: &crate::line::Line) -> bool {
    let flags = line.flags;
    flags.contains(crate::line::LineFlags::COLS)
        || flags.contains(crate::line::LineFlags::TABS)
        || flags.contains(crate::line::LineFlags::BOUNDS)
        || flags.contains(crate::line::LineFlags::MASK)
        || flags.contains(crate::line::LineFlags::PROF)
        || flags.contains(crate::line::LineFlags::WORD)
        || flags.contains(crate::line::LineFlags::MARK)
        || line.is_note()
}

fn swapbar(session: &mut Session, rest: &str) -> EditResult<String> {
    match rest.trim().to_ascii_uppercase().as_str() {
        "" => {
            let on = session.toggle_swapbar();
            Ok(if on { "SWAPBAR ON" } else { "SWAPBAR OFF" }.to_string())
        }
        "ON" => {
            session.set_swapbar(true);
            Ok("SWAPBAR ON".to_string())
        }
        "OFF" => {
            session.set_swapbar(false);
            Ok("SWAPBAR OFF".to_string())
        }
        "/" => Err(EditError::new("Not supported")),
        other => Err(EditError::new(format!("Unknown keyword '{other}'"))),
    }
}

fn start_screen(session: &mut Session, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new("START takes no operands"));
    }
    let index = session.add_screen();
    let title = session.tabs()[index].title.clone();
    Ok(format!("Screen {} {title}", index + 1))
}

fn swap(session: &mut Session, rest: &str) -> EditResult<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return swap_other(session);
    }
    let upper = rest.to_ascii_uppercase();
    match upper.as_str() {
        "LIST" => {
            session.set_swap_list(true);
            Ok("SWAP LIST".to_string())
        }
        "NEXT" => swap_neighbor(session, true),
        "PREV" => swap_neighbor(session, false),
        _ => {
            if let Ok(number) = rest.parse::<usize>() {
                if number == 0 || number > session.tabs().len() {
                    return Err(EditError::new(format!("Screen {number} is not active")));
                }
                return show_screen(session, number - 1);
            }
            let index = find_screen(session, rest)?;
            show_screen(session, index)
        }
    }
}

fn swap_other(session: &mut Session) -> EditResult<String> {
    if session.tabs().len() < 2 {
        return Ok("No other screen".to_string());
    }
    let index = session
        .alternate()
        .unwrap_or_else(|| neighbor(session.tabs().len(), session.current_index(), true));
    show_screen(session, index)
}

fn swap_neighbor(session: &mut Session, next: bool) -> EditResult<String> {
    if session.tabs().len() < 2 {
        return Ok("No other screen".to_string());
    }
    let index = neighbor(session.tabs().len(), session.current_index(), next);
    show_screen(session, index)
}

fn show_screen(session: &mut Session, index: usize) -> EditResult<String> {
    let title = session
        .tabs()
        .get(index)
        .map(|tab| tab.title.clone())
        .ok_or_else(|| EditError::new(format!("Screen {} is not active", index + 1)))?;
    session.activate_screen(index)?;
    Ok(format!("Screen {} {title}", index + 1))
}

fn neighbor(len: usize, current: usize, next: bool) -> usize {
    if len <= 1 {
        return current;
    }
    if next {
        if current + 1 >= len {
            0
        } else {
            current + 1
        }
    } else if current == 0 {
        len - 1
    } else {
        current - 1
    }
}

fn find_screen(session: &Session, name: &str) -> EditResult<usize> {
    let exact: Vec<usize> = session
        .tabs()
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.title.eq_ignore_ascii_case(name))
        .map(|(index, _)| index)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    if exact.len() > 1 {
        return Err(EditError::new(format!("Screen '{name}' is not unique")));
    }
    if (2..=8).contains(&name.chars().count()) {
        let prefix: Vec<usize> = session
            .tabs()
            .iter()
            .enumerate()
            .filter(|(_, tab)| {
                let short: String = tab.title.chars().take(8).collect();
                short.eq_ignore_ascii_case(name) && tab.title.chars().count() > 8
            })
            .map(|(index, _)| index)
            .collect();
        if prefix.len() == 1 {
            return Ok(prefix[0]);
        }
        if prefix.len() > 1 {
            return Err(EditError::new(format!("Screen '{name}' is not unique")));
        }
    }
    Err(EditError::new(format!("Screen '{name}' is not active")))
}

fn save(session: &mut Session, rest: &str) -> EditResult<String> {
    let rest = rest.trim();
    let path = if rest.is_empty() {
        None
    } else {
        Some(unwrap_quotes(rest))
    };
    let saved = session.save_current(path.as_deref())?;
    Ok(format!("Saved {}", saved.display()))
}

fn open(session: &mut Session, rest: &str) -> EditResult<String> {
    let path = unwrap_quotes(rest.trim());
    if path.is_empty() {
        return Err(EditError::new("OPEN needs a file name"));
    }
    session.open_path(std::path::Path::new(&path))?;
    let tab = session.current();
    Ok(format!(
        "Editing {} ({} lines)",
        tab.title,
        tab.doc.last_real()
    ))
}

fn encoding(session: &mut Session, rest: &str) -> EditResult<String> {
    let name = rest.trim();
    if name.is_empty() {
        return Ok(format!(
            "Encoding is {}",
            session.current().doc.encoding().name()
        ));
    }
    let encoding = Encoding::parse(name)
        .ok_or_else(|| EditError::new("Encoding must be UTF8, UTF8BOM, or ANSI"))?;
    session.current_mut().doc.set_encoding(encoding);
    Ok(format!("Encoding set to {}", encoding.name()))
}

fn switch_tab(session: &mut Session, rest: &str) -> EditResult<String> {
    let n: usize = rest
        .trim()
        .parse()
        .map_err(|_| EditError::new("TAB needs a tab number"))?;
    session.select(
        n.checked_sub(1)
            .ok_or_else(|| EditError::new("TAB number starts at 1"))?,
    )?;
    Ok(format!("Tab {n}: {}", session.current().title))
}

fn line_at(doc: &mut Document, rest: &str) -> EditResult<String> {
    let (reference, command) = split_token(rest)?;
    let line = doc.resolve_line(reference)?;
    let outcome = apply_prefixes(doc, &[(line, command.to_string())])?;
    if outcome.stop && outcome.done.is_empty() && !outcome.message.starts_with("Pending") {
        return Err(EditError::new(outcome.message));
    }
    Ok(outcome.message)
}

/// Prefix commands from one Enter. `done` is the prefix lines to clear.
/// `stop` means the command line should not run: a command is still pending,
/// or a later prefix failed after earlier ones were applied.
pub(crate) struct PrefixOutcome {
    pub message: String,
    pub done: Vec<usize>,
    pub stop: bool,
    /// The cursor is already on the line that should remain. Enter does not move down.
    pub hold_line: bool,
    /// The cursor stays in the data at the column already chosen.
    pub data_cursor: bool,
}

pub(crate) fn apply_prefixes(
    doc: &mut Document,
    commands: &[(usize, String)],
) -> EditResult<PrefixOutcome> {
    let mut bookmark_done = Vec::new();
    let mut bookmark_message = String::new();
    let mut regular = Vec::new();
    for (line, command) in commands {
        if doc.line(*line)?.is_invisible() {
            return Err(EditError::new("Line is excluded"));
        }
        if command.trim().starts_with('.') {
            bookmark_message = apply_bookmark(doc, *line, command)?;
            bookmark_done.push(*line);
        } else {
            regular.push((*line, command));
        }
    }
    if regular.is_empty() {
        return Ok(PrefixOutcome {
            message: bookmark_message,
            done: bookmark_done,
            stop: false,
            hold_line: false,
            data_cursor: false,
        });
    }

    let mut parsed = Vec::with_capacity(regular.len());
    for (line, command) in regular {
        let item = parse_line_command(command)?;
        parsed.push((line, item));
    }

    let overridden = parsed.iter().any(|(line, _)| doc.pending().covers(*line));
    let (marks, ordinary): (Vec<_>, Vec<_>) = parsed
        .into_iter()
        .partition(|(_, item)| item.kind.is_relocate());

    if marks.is_empty() {
        if doc.pending().is_relocate() && !overridden {
            return Ok(PrefixOutcome {
                message: if bookmark_message.is_empty() {
                    pending_status(doc.pending())
                } else {
                    bookmark_message
                },
                done: bookmark_done,
                stop: true,
                hold_line: false,
                data_cursor: false,
            });
        }
        if overridden && doc.pending().is_relocate() {
            doc.set_pending(PendingBlock::None);
        }
        let mut outcome = run_ordinary(doc, ordinary, bookmark_done)?;
        if outcome.message.is_empty() {
            outcome.message = bookmark_message;
        }
        return Ok(outcome);
    }

    match finish_relocate(doc, &marks, overridden)? {
        RelocateStep::Stop(message) => {
            let mut done: Vec<usize> = marks.into_iter().map(|(line, _)| line).collect();
            done.extend(bookmark_done);
            Ok(PrefixOutcome {
                message: if message.is_empty() {
                    bookmark_message
                } else {
                    message
                },
                done,
                stop: true,
                hold_line: false,
                data_cursor: false,
            })
        }
        RelocateStep::Applied { message, shift } => {
            let mut done: Vec<usize> = marks.into_iter().map(|(line, _)| line).collect();
            done.extend(bookmark_done);
            let mut shifted = Vec::new();
            for (key, item) in ordinary {
                match shift.map(key) {
                    Some(line) => shifted.push((key, line, item)),
                    None => done.push(key),
                }
            }
            let mut outcome = run_ordinary_shifted(doc, shifted, done)?;
            if outcome.message.is_empty() {
                outcome.message = message;
            }
            Ok(outcome)
        }
    }
}

fn run_ordinary(
    doc: &mut Document,
    commands: Vec<(usize, ParsedLine)>,
    done: Vec<usize>,
) -> EditResult<PrefixOutcome> {
    let shifted = commands
        .into_iter()
        .map(|(line, item)| (line, line, item))
        .collect();
    run_ordinary_shifted(doc, shifted, done)
}

fn run_ordinary_shifted(
    doc: &mut Document,
    mut commands: Vec<(usize, usize, ParsedLine)>,
    mut done: Vec<usize>,
) -> EditResult<PrefixOutcome> {
    commands.sort_by_key(|(_, line, _)| std::cmp::Reverse(*line));
    let mut message = String::new();
    let mut hold_line = false;
    let mut data_cursor = false;
    for (key, line, item) in commands {
        match apply_parsed(doc, line, item) {
            Ok(text) => {
                done.push(key);
                hold_line = matches!(item.kind, LineKind::Delete | LineKind::Insert);
                data_cursor = item.kind == LineKind::Insert;
                if !text.is_empty() {
                    message = text;
                }
            }
            Err(err) => {
                return Ok(PrefixOutcome {
                    message: err.message,
                    done,
                    stop: true,
                    hold_line: false,
                    data_cursor: false,
                });
            }
        }
    }
    Ok(PrefixOutcome {
        message,
        done,
        stop: false,
        hold_line,
        data_cursor,
    })
}

fn column_under_previous_word(doc: &Document, line: usize) -> usize {
    let mut at = line;
    while at > 1 {
        at -= 1;
        let Ok(row) = doc.line(at) else {
            break;
        };
        if !row.is_data() && !row.is_note() {
            continue;
        }
        if let Some(index) = row.chars().iter().position(|ch| !ch.is_whitespace()) {
            return index + 1;
        }
    }
    1
}

/// Enter on an `I` line that already has text opens another insert line after it.
pub(crate) fn reopen_insert(doc: &mut Document) -> EditResult<bool> {
    let line = doc.cursor().0;
    let row = doc.line(line)?;
    if !row.is_insert() || row.text().trim().is_empty() {
        return Ok(false);
    }
    let first = doc.insert_lines_after(line, 1, true)?;
    doc.clear_insert_flag(line);
    let col = column_under_previous_word(doc, first);
    doc.set_cursor(first, col)?;
    Ok(true)
}

fn apply_parsed(doc: &mut Document, line: usize, parsed: ParsedLine) -> EditResult<String> {
    if doc.pending().covers(line) && !continues_block(doc.pending(), parsed) {
        doc.set_pending(PendingBlock::None);
    }
    match (parsed.kind, parsed.block) {
        (LineKind::Delete, false) => {
            if doc.line(line)?.is_xclude() {
                // `D` on a marker deletes the whole displayed block.
            }
            doc.delete_units(line, parsed.repeat)?;
            doc.set_cursor(line.min(doc.last_line()), 1)?;
            Ok("Deleted".to_string())
        }
        (LineKind::Delete, true) => block_delete(doc, line),
        (LineKind::Insert, false) => {
            let first = doc.insert_lines_after(line, parsed.repeat, true)?;
            let col = column_under_previous_word(doc, first);
            doc.set_cursor(first, col)?;
            Ok(format!(
                "Inserted {} line{}",
                parsed.repeat,
                plural(parsed.repeat)
            ))
        }
        (LineKind::Exclude, false) => {
            if doc.line(line)?.is_xclude() {
                return Err(EditError::new("Line is already excluded"));
            }
            let end = doc.span_end(line, parsed.repeat)?;
            let n = doc.exclude_range(line, end)?;
            Ok(format!("Excluded {n} line{}", plural(n)))
        }
        (LineKind::Exclude, true) => block_exclude(doc, line),
        (LineKind::First, false) => {
            doc.show_first(line, parsed.repeat)?;
            doc.set_cursor(line, 1)?;
            Ok("Excluded lines shown".to_string())
        }
        (LineKind::Last, false) => {
            doc.show_last(line, parsed.repeat)?;
            doc.set_cursor(line, 1)?;
            Ok("Excluded lines shown".to_string())
        }
        (LineKind::Show, false) => {
            let count = doc.line(line)?.work1.max(0) as usize;
            doc.show_first(line, count.max(1))?;
            doc.set_cursor(line, 1)?;
            Ok("Excluded lines shown".to_string())
        }
        (LineKind::Insert, true)
        | (LineKind::First, true)
        | (LineKind::Last, true)
        | (LineKind::Show, true) => Err(EditError::new("That line command has no block form")),
        (LineKind::Repeat, false) => repeat_line(doc, line, parsed.repeat),
        (LineKind::Repeat, true) => block_repeat(doc, line, parsed.repeat),
        (LineKind::Upper, false) => {
            let end = doc.span_end(line, parsed.repeat)?;
            change_case(doc, line, end, true)
        }
        (LineKind::Lower, false) => {
            let end = doc.span_end(line, parsed.repeat)?;
            change_case(doc, line, end, false)
        }
        (LineKind::Upper, true) => block_case(doc, line, true),
        (LineKind::Lower, true) => block_case(doc, line, false),
        (LineKind::ColLeft, false) => shift_span(doc, line, parsed.repeat, false, false),
        (LineKind::ColRight, false) => shift_span(doc, line, parsed.repeat, true, false),
        (LineKind::DataLeft, false) => shift_span(doc, line, parsed.repeat, false, true),
        (LineKind::DataRight, false) => shift_span(doc, line, parsed.repeat, true, true),
        (LineKind::ColLeft, true) => block_shift(doc, line, parsed.repeat, false, false),
        (LineKind::ColRight, true) => block_shift(doc, line, parsed.repeat, true, false),
        (LineKind::DataLeft, true) => block_shift(doc, line, parsed.repeat, false, true),
        (LineKind::DataRight, true) => block_shift(doc, line, parsed.repeat, true, true),
        (LineKind::Copy | LineKind::Move | LineKind::After | LineKind::Before, _)
        | (
            LineKind::Overlay
            | LineKind::OverlayReplace
            | LineKind::Here
            | LineKind::AfterEach
            | LineKind::BeforeEach
            | LineKind::Swap,
            _,
        ) => Err(EditError::new(
            "Copy and move are handled with their destination",
        )),
        (kind, block) => crate::more::line_action(doc, line, line_op(kind), block, parsed.repeat),
    }
}

fn line_op(kind: LineKind) -> crate::more::LineOp {
    match kind {
        LineKind::Join => crate::more::LineOp::Join,
        LineKind::Glue => crate::more::LineOp::Glue,
        LineKind::TextJoin => crate::more::LineOp::TextJoin,
        LineKind::TextGlue => crate::more::LineOp::TextGlue,
        LineKind::Sentence => crate::more::LineOp::Sentence,
        LineKind::Title => crate::more::LineOp::Title,
        LineKind::Trim => crate::more::LineOp::Trim,
        LineKind::Pad => crate::more::LineOp::Pad,
        LineKind::Trunc => crate::more::LineOp::Trunc,
        LineKind::IndentLeft => crate::more::LineOp::IndentLeft,
        LineKind::IndentRight => crate::more::LineOp::IndentRight,
        LineKind::Split => crate::more::LineOp::Split,
        LineKind::Flow => crate::more::LineOp::Flow,
        LineKind::Margin => crate::more::LineOp::Margin,
        LineKind::Note => crate::more::LineOp::Note,
        LineKind::MakeNote => crate::more::LineOp::MakeNote,
        LineKind::MakeData => crate::more::LineOp::MakeData,
        LineKind::User => crate::more::LineOp::User,
        LineKind::Revert => crate::more::LineOp::Revert,
        LineKind::ToggleX => crate::more::LineOp::ToggleX,
        LineKind::ToggleUser => crate::more::LineOp::ToggleUser,
        LineKind::Cols => crate::more::LineOp::Cols,
        LineKind::TabsLine => crate::more::LineOp::Tabs,
        LineKind::Bnds => crate::more::LineOp::Bnds,
        LineKind::Mask => crate::more::LineOp::Mask,
        LineKind::Word => crate::more::LineOp::Word,
        LineKind::Mark => crate::more::LineOp::Mark,
        LineKind::Text => crate::more::LineOp::Text,
        _ => crate::more::LineOp::Text,
    }
}

fn delete_matches(doc: &mut Document, rest: &str) -> EditResult<String> {
    let lines = matching_lines(doc, rest)?;
    let n = doc.delete_lines(&lines)?;
    Ok(format!("Deleted {n} line{}", plural(n)))
}

fn exclude_matches(doc: &mut Document, rest: &str) -> EditResult<String> {
    // ISPF: EXCLUDE ALL with no search string hides every line. The lines stay in the file.
    if rest.trim().eq_ignore_ascii_case("ALL") {
        let lines: Vec<usize> = (1..doc.last_line())
            .filter(|&n| doc.line(n).is_ok_and(|row| row.is_data() || row.is_note()))
            .collect();
        let n = doc.exclude_lines(&lines)?;
        return Ok(format!("Excluded {n} line{}", plural(n)));
    }
    let lines = matching_lines(doc, rest)?;
    let n = doc.exclude_lines(&lines)?;
    Ok(format!("Excluded {n} line{}", plural(n)))
}

fn matching_lines(doc: &Document, rest: &str) -> EditResult<Vec<usize>> {
    let (needle, which, _) = parse_search(doc, rest)?;
    if which == Which::All {
        let mut lines = Vec::new();
        for line in 1..doc.last_line() {
            if !match_columns(doc, line, &needle).is_empty() {
                lines.push(line);
            }
        }
        if lines.is_empty() {
            return Err(not_found(&needle));
        }
        return Ok(lines);
    }
    let hit = seek(doc, &needle, which, false, None)?;
    Ok(vec![hit.line])
}

fn apply_bookmark(doc: &mut Document, line: usize, text: &str) -> EditResult<String> {
    let name = text.trim().to_ascii_uppercase();
    if name == "." {
        doc.set_label(line, None)?;
        return Ok("Bookmark cleared".to_string());
    }
    let name = normalize_bookmark(&name)?;
    if doc.line(line)?.label.as_deref() == Some(name.as_str()) {
        doc.set_label(line, None)?;
        return Ok(format!("Bookmark {name} cleared"));
    }
    doc.set_label(line, Some(&name))?;
    Ok(format!("Bookmark {name} set"))
}

fn bookmark(doc: &mut Document, rest: &str) -> EditResult<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        let line = doc.cursor().0;
        if let Some(name) = doc.line(line)?.label.clone() {
            return Ok(format!("Bookmark {name}"));
        }
        let name = next_bookmark(doc)?;
        doc.set_label(line, Some(&name))?;
        return Ok(format!("Bookmark {name} set"));
    }
    if rest.eq_ignore_ascii_case("LIST") || rest == "?" {
        return Ok(bookmark_list(doc));
    }
    let line = doc.cursor().0;
    let name = normalize_bookmark(rest)?;
    doc.set_label(line, Some(&name))?;
    Ok(format!("Bookmark {name} set"))
}

fn next_bookmark(doc: &Document) -> EditResult<String> {
    for ch in 'A'..='Z' {
        let name = format!(".{ch}");
        let used = (1..doc.last_line()).any(|line| {
            doc.line(line).ok().and_then(|row| row.label.as_deref()) == Some(name.as_str())
        });
        if !used {
            return Ok(name);
        }
    }
    Err(EditError::new("No free bookmark from .A to .Z"))
}

fn bookmark_list(doc: &Document) -> String {
    let mut marks = Vec::new();
    for line in 1..doc.last_line() {
        if let Some(name) = doc.line(line).ok().and_then(|row| row.label.clone()) {
            let seq = doc.line(line).map(|row| row.seq).unwrap_or(0);
            marks.push(format!("{name} line {seq}"));
        }
    }
    if marks.is_empty() {
        "No bookmarks".to_string()
    } else {
        format!("Bookmarks: {}", marks.join(", "))
    }
}

fn normalize_bookmark(raw: &str) -> EditResult<String> {
    let text = raw.trim().to_ascii_uppercase();
    let name = if text.starts_with('.') {
        text
    } else {
        format!(".{text}")
    };
    if matches!(name.as_str(), ".ZF" | ".ZL" | ".ZFIRST" | ".ZLAST") {
        return Err(EditError::new(format!("{name} is reserved")));
    }
    if name.chars().count() > 8 {
        return Err(EditError::new("Name is longer than 8 characters"));
    }
    let body = name.trim_start_matches('.');
    let mut chars = body.chars();
    let Some(first) = chars.next() else {
        return Err(EditError::new("Name is empty"));
    };
    if first.is_ascii_digit() || !first.is_ascii_alphabetic() {
        return Err(EditError::new("Name cannot start with a digit"));
    }
    if body.chars().any(|ch| !ch.is_ascii_alphanumeric()) {
        return Err(EditError::new("Name contains an invalid character"));
    }
    Ok(name)
}

fn scroll_edge(doc: &mut Document, top: bool, rest: &str) -> EditResult<String> {
    if !rest.trim().is_empty() {
        return Err(EditError::new(if top {
            "TOP takes no operands"
        } else {
            "BOTTOM takes no operands"
        }));
    }
    let line = if top { 1 } else { doc.last_line().max(1) };
    let col = doc.cursor().1;
    doc.set_cursor(line, col)?;
    Ok(if top {
        "Top of data".to_string()
    } else {
        "Bottom of data".to_string()
    })
}

fn sort_cmd(doc: &mut Document, rest: &str) -> EditResult<String> {
    let word = rest
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let descending = match word.as_str() {
        "" | "A" | "ASC" | "ASCENDING" => false,
        "D" | "DESC" | "DESCENDING" => true,
        _ => return Err(EditError::new("SORT direction must be A or D")),
    };
    doc.sort_data(descending)?;
    Ok(if descending {
        "Sorted descending".to_string()
    } else {
        "Sorted".to_string()
    })
}

fn repeat_line(doc: &mut Document, line: usize, times: usize) -> EditResult<String> {
    ensure_editable(doc, line)?;
    let times = times.max(1);
    doc.copy_move(line, line, line, false, times, false)?;
    Ok(format!("Repeated {times}"))
}

fn block_repeat(doc: &mut Document, line: usize, times: usize) -> EditResult<String> {
    match doc.pending() {
        PendingBlock::Replicate {
            line: other,
            times: prev,
        } => {
            doc.set_pending(PendingBlock::None);
            if other == line {
                return Ok("Repeat cancelled".to_string());
            }
            let (start, end) = expand_block(doc, other, line)?;
            let times = merge_repeat(prev, times)?.max(1);
            doc.copy_move(start, end, end, false, times, false)?;
            Ok(format!("Repeated {times}"))
        }
        PendingBlock::None => {
            doc.set_pending(PendingBlock::Replicate { line, times });
            Ok("RR pending".to_string())
        }
        other => Err(other_block(other)),
    }
}

fn change_case(doc: &mut Document, start: usize, end: usize, upper: bool) -> EditResult<String> {
    doc.map_text(start, end, |text| {
        if upper {
            text.to_uppercase()
        } else {
            text.to_lowercase()
        }
    })?;
    doc.set_cursor(start, 1)?;
    Ok(if upper {
        "Uppercased".to_string()
    } else {
        "Lowercased".to_string()
    })
}

fn block_case(doc: &mut Document, line: usize, upper: bool) -> EditResult<String> {
    let pending = doc.pending();
    let partner = match pending {
        PendingBlock::Upper(other) if upper => Some(other),
        PendingBlock::Lower(other) if !upper => Some(other),
        PendingBlock::None => None,
        other => return Err(other_block(other)),
    };
    match partner {
        Some(other) => {
            doc.set_pending(PendingBlock::None);
            if other == line {
                return Ok(if upper {
                    "Uppercase cancelled".to_string()
                } else {
                    "Lowercase cancelled".to_string()
                });
            }
            let (start, end) = expand_block(doc, other, line)?;
            change_case(doc, start, end, upper)
        }
        None => {
            doc.set_pending(if upper {
                PendingBlock::Upper(line)
            } else {
                PendingBlock::Lower(line)
            });
            Ok(if upper {
                "UCC pending".to_string()
            } else {
                "LCC pending".to_string()
            })
        }
    }
}

fn shift_span(
    doc: &mut Document,
    line: usize,
    cols: usize,
    right: bool,
    data: bool,
) -> EditResult<String> {
    ensure_editable(doc, line)?;
    let cols = if cols == 0 { 2 } else { cols };
    doc.map_text(line, line, |text| shift_columns(text, cols, right, data))?;
    doc.set_cursor(line, 1)?;
    Ok(format!("Shifted {cols}"))
}

fn block_shift(
    doc: &mut Document,
    line: usize,
    cols: usize,
    right: bool,
    data: bool,
) -> EditResult<String> {
    match doc.pending() {
        PendingBlock::Shift {
            line: other,
            cols: prev,
            right: pending_right,
            data: pending_data,
        } if pending_right == right && pending_data == data => {
            doc.set_pending(PendingBlock::None);
            if other == line {
                return Ok("Shift cancelled".to_string());
            }
            let (start, end) = expand_block(doc, other, line)?;
            let cols = shift_count(prev, cols)?;
            doc.map_text(start, end, |text| shift_columns(text, cols, right, data))?;
            doc.set_cursor(start, 1)?;
            Ok(format!("Shifted {cols}"))
        }
        PendingBlock::None => {
            doc.set_pending(PendingBlock::Shift {
                line,
                cols,
                right,
                data,
            });
            Ok("Shift pending".to_string())
        }
        other => Err(other_block(other)),
    }
}

fn shift_count(left: usize, right: usize) -> EditResult<usize> {
    let merged = merge_repeat(left, right)?;
    Ok(if merged == 0 { 2 } else { merged })
}

/// Column shift inserts or deletes at column 1. Data shift does the same at
/// the first non-blank, and a left data shift spends its count on the indent
/// before it touches the text.
fn shift_columns(text: &str, cols: usize, right: bool, data: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    if cols == 0 {
        return text.to_string();
    }
    if !data {
        if right {
            let mut out = vec![' '; cols];
            out.extend(chars);
            return out.into_iter().collect();
        }
        let start = cols.min(chars.len());
        return chars[start..].iter().collect();
    }
    let indent = chars.iter().take_while(|ch| ch.is_whitespace()).count();
    if right {
        let mut out = Vec::with_capacity(chars.len() + cols);
        out.extend_from_slice(&chars[..indent]);
        out.extend(std::iter::repeat(' ').take(cols));
        out.extend_from_slice(&chars[indent..]);
        return out.into_iter().collect();
    }
    let from_indent = cols.min(indent);
    let from_text = cols - from_indent;
    let start = (from_indent + from_text).min(chars.len());
    chars[start..].iter().collect()
}

fn ensure_editable(doc: &Document, line: usize) -> EditResult<()> {
    let row = doc.line(line)?;
    if row.is_top() || row.is_bottom() || row.is_xclude() {
        return Err(EditError::new("Line command needs a data line"));
    }
    Ok(())
}

fn continues_block(pending: PendingBlock, parsed: ParsedLine) -> bool {
    if !parsed.block {
        return false;
    }
    match (pending, parsed.kind) {
        (PendingBlock::Delete(_), LineKind::Delete)
        | (PendingBlock::Exclude(_), LineKind::Exclude)
        | (PendingBlock::Replicate { .. }, LineKind::Repeat)
        | (PendingBlock::Upper(_), LineKind::Upper)
        | (PendingBlock::Lower(_), LineKind::Lower) => true,
        (PendingBlock::Pair { kind, .. }, line_kind) if same_pair(line_kind, kind) => true,
        (
            PendingBlock::Shift {
                right, data: true, ..
            },
            LineKind::DataRight,
        ) if right => true,
        (
            PendingBlock::Shift {
                right, data: true, ..
            },
            LineKind::DataLeft,
        ) if !right => true,
        (
            PendingBlock::Shift {
                right, data: false, ..
            },
            LineKind::ColRight,
        ) if right => true,
        (
            PendingBlock::Shift {
                right, data: false, ..
            },
            LineKind::ColLeft,
        ) if !right => true,
        _ => false,
    }
}

fn same_pair(kind: LineKind, pair: crate::document::PairKind) -> bool {
    line_op(kind).pair() == pair && line_op(kind).pairs()
}

fn other_block(pending: PendingBlock) -> EditError {
    let message = match pending {
        PendingBlock::Delete(_) => "A DD block is still open",
        PendingBlock::Exclude(_) => "An XX block is still open",
        PendingBlock::Replicate { .. } => "An RR block is still open",
        PendingBlock::Upper(_) => "A UCC block is still open",
        PendingBlock::Lower(_) => "An LCC block is still open",
        PendingBlock::Shift { .. } => "A shift block is still open",
        PendingBlock::Pair { kind, .. } => {
            return EditError::new(format!(
                "A {} block is still open",
                crate::document::pair_name(kind)
            ));
        }
        PendingBlock::DestOpen { kind, .. } => {
            return EditError::new(format!(
                "A {} block is still open",
                crate::document::dest_name(kind, true)
            ));
        }
        _ if pending.is_relocate() => "A copy or move is still pending",
        _ => "A block line command is still open",
    };
    EditError::new(message)
}

fn block_delete(doc: &mut Document, line: usize) -> EditResult<String> {
    match doc.pending() {
        pending if pending.is_relocate() => Err(EditError::new("A copy or move is still pending")),
        PendingBlock::Exclude(_) => Err(EditError::new("An XX block is still open")),
        PendingBlock::Delete(other) => {
            doc.set_pending(PendingBlock::None);
            if other == line {
                return Ok("Delete cancelled".to_string());
            }
            let (start, end) = expand_block(doc, other, line)?;
            doc.delete_range(start, end)?;
            doc.set_cursor(start.min(doc.last_line()), 1)?;
            Ok("Deleted".to_string())
        }
        PendingBlock::None => {
            doc.set_pending(PendingBlock::Delete(line));
            Ok("DD pending".to_string())
        }
        other => Err(other_block(other)),
    }
}

fn block_exclude(doc: &mut Document, line: usize) -> EditResult<String> {
    match doc.pending() {
        pending if pending.is_relocate() => Err(EditError::new("A copy or move is still pending")),
        PendingBlock::Delete(_) => Err(EditError::new("A DD block is still open")),
        PendingBlock::Exclude(other) => {
            doc.set_pending(PendingBlock::None);
            if other == line {
                return Ok("Exclude cancelled".to_string());
            }
            let (start, end) = expand_block(doc, other, line)?;
            let n = doc.exclude_range(start, end)?;
            Ok(format!("Excluded {n} line{}", plural(n)))
        }
        PendingBlock::None => {
            doc.set_pending(PendingBlock::Exclude(line));
            Ok("XX pending".to_string())
        }
        other => Err(other_block(other)),
    }
}

fn expand_block(doc: &Document, a: usize, b: usize) -> EditResult<(usize, usize)> {
    doc.line(a)?;
    doc.line(b)?;
    if doc.line(a)?.is_invisible() || doc.line(b)?.is_invisible() {
        return Err(EditError::new("Block command is on a hidden line"));
    }
    let start = a.min(b);
    let mut end = a.max(b);
    if doc.line(end)?.is_xclude() {
        end += doc.line(end)?.work1.max(0) as usize;
    }
    if end > doc.last_line() {
        end = doc.last_line() - 1;
    }
    if start >= doc.last_line() {
        return Err(EditError::new("Block is outside the data"));
    }
    Ok((start, end))
}

enum RelocateStep {
    Stop(String),
    Applied { message: String, shift: LineShift },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkKind {
    Copy,
    BlockCopy,
    Move,
    BlockMove,
    After,
    Before,
    Overlay,
    BlockOverlay,
    OverlayReplace,
    BlockOverlayReplace,
    Here,
    BlockHere,
    AfterEach,
    BeforeEach,
    Swap,
    BlockSwap,
}

#[derive(Clone, Copy)]
struct Mark {
    line: usize,
    kind: MarkKind,
    number: usize,
    user: bool,
}

impl MarkKind {
    fn is_source(self) -> bool {
        matches!(
            self,
            MarkKind::Copy | MarkKind::BlockCopy | MarkKind::Move | MarkKind::BlockMove
        )
    }

    fn is_move(self) -> bool {
        matches!(self, MarkKind::Move | MarkKind::BlockMove)
    }

    fn is_block(self) -> bool {
        matches!(self, MarkKind::BlockCopy | MarkKind::BlockMove)
    }

    fn is_block_dest(self) -> bool {
        matches!(
            self,
            MarkKind::BlockOverlay
                | MarkKind::BlockOverlayReplace
                | MarkKind::BlockHere
                | MarkKind::AfterEach
                | MarkKind::BeforeEach
                | MarkKind::BlockSwap
        )
    }

    fn is_plain_dest(self) -> bool {
        matches!(self, MarkKind::After | MarkKind::Before)
    }
}

fn finish_relocate(
    doc: &mut Document,
    typed: &[(usize, ParsedLine)],
    overridden: bool,
) -> EditResult<RelocateStep> {
    match doc.pending() {
        PendingBlock::Delete(_) if !overridden => {
            return Err(EditError::new("A DD block is still open"));
        }
        PendingBlock::Exclude(_) if !overridden => {
            return Err(EditError::new("An XX block is still open"));
        }
        pending
            if !overridden && !matches!(pending, PendingBlock::None) && !pending.is_relocate() =>
        {
            return Err(other_block(pending));
        }
        _ => {}
    }

    let mut marks: Vec<Mark> = typed
        .iter()
        .map(|(line, item)| Mark {
            line: *line,
            kind: mark_kind(*item),
            number: item.repeat,
            user: true,
        })
        .collect();
    if !overridden {
        marks.extend(pending_marks(doc.pending()));
    }
    marks.sort_by_key(|mark| mark.line);

    let sources: Vec<&Mark> = marks.iter().filter(|mark| mark.kind.is_source()).collect();
    let dests: Vec<&Mark> = marks.iter().filter(|mark| !mark.kind.is_source()).collect();
    if dests.len() > 2 || sources.len() > 2 {
        return Err(EditError::new("Illogical line command grouping"));
    }
    if dests.len() == 2
        && (!dests[0].kind.is_block_dest()
            || dests[0].kind != dests[1].kind
            || dests[0].line == dests[1].line)
    {
        return Err(EditError::new("Illogical line command grouping"));
    }
    if sources.len() == 2 && (!sources[0].kind.is_block() || sources[0].kind != sources[1].kind) {
        return Err(EditError::new("Illogical line command grouping"));
    }
    if sources.len() == 2 && sources[0].line == sources[1].line {
        return Err(EditError::new("Illogical line command grouping"));
    }

    if dests.is_empty()
        && sources.len() == 1
        && sources[0].user
        && cancels(doc.pending(), sources[0])
    {
        doc.set_pending(PendingBlock::None);
        let message = if sources[0].kind.is_move() {
            "Move cancelled"
        } else {
            "Copy cancelled"
        };
        return Ok(RelocateStep::Applied {
            message: message.to_string(),
            shift: LineShift::identity(),
        });
    }
    if sources.is_empty() && dests.len() == 1 && dests[0].user && cancels(doc.pending(), dests[0]) {
        doc.set_pending(PendingBlock::None);
        return Ok(RelocateStep::Applied {
            message: "Destination cancelled".to_string(),
            shift: LineShift::identity(),
        });
    }

    let source = match sources.as_slice() {
        [] => None,
        [one] if one.kind.is_block() => {
            if !dests.is_empty() {
                let name = if one.kind.is_move() { "MM" } else { "CC" };
                return Err(EditError::new(format!("{name} block is still open")));
            }
            doc.set_pending(open_block(one));
            return Ok(RelocateStep::Stop("Pending command(s)".to_string()));
        }
        [one] => Some(one_source(doc, one)?),
        [left, right] => Some(block_source(doc, left, right)?),
        _ => return Err(EditError::new("Illogical line command grouping")),
    };
    let dest = match dests.as_slice() {
        [] => None,
        [one] if one.kind.is_block_dest() => {
            if !sources.is_empty() {
                return Err(EditError::new(format!(
                    "{} block is still open",
                    dest_label(one.kind)
                )));
            }
            doc.set_pending(PendingBlock::DestOpen {
                line: one.line,
                kind: to_dest_kind(one.kind),
                repeat: one.number,
            });
            return Ok(RelocateStep::Stop("Pending command(s)".to_string()));
        }
        [one] => Some(DestAt {
            start: one.line,
            end: one.line,
            kind: one.kind,
            number: one.number,
        }),
        [left, right] => {
            let (start, end) = if left.line < right.line {
                (left.line, right.line)
            } else {
                (right.line, left.line)
            };
            Some(DestAt {
                start,
                end,
                kind: left.kind,
                number: merge_repeat(left.number, right.number)?,
            })
        }
        _ => return Err(EditError::new("Illogical line command grouping")),
    };

    match (source, dest) {
        (None, None) => Ok(RelocateStep::Stop(String::new())),
        (Some(source), None) => {
            doc.set_pending(source_pending(source));
            Ok(RelocateStep::Stop(
                "Pending destination line command".to_string(),
            ))
        }
        (None, Some(dest)) => {
            doc.set_pending(store_dest(dest));
            Ok(RelocateStep::Stop(
                "Pending source line command".to_string(),
            ))
        }
        (Some(source), Some(dest)) => {
            if source.factor > 0 && dest.number > 0 && dest.kind.is_plain_dest() {
                return Err(EditError::new("Multiple repeat values entered"));
            }
            if dest.kind.is_plain_dest() {
                let times = source.factor.max(dest.number).max(1);
                let before = dest.kind == MarkKind::Before;
                let shift = doc.copy_move(
                    source.start,
                    source.end,
                    dest.start,
                    before,
                    times,
                    source.delete_source,
                )?;
                let message = if source.delete_source {
                    "Moved"
                } else {
                    "Copied"
                };
                return Ok(RelocateStep::Applied {
                    message: message.to_string(),
                    shift,
                });
            }
            let (dest_start, dest_end) = if dest.start == dest.end
                && matches!(dest.kind, MarkKind::Overlay | MarkKind::OverlayReplace)
            {
                let count = source.end - source.start + 1;
                (dest.start, doc.span_end(dest.start, count)?)
            } else {
                (dest.start, dest.end)
            };
            let shift = crate::more::apply_dest(
                doc,
                source.start,
                source.end,
                dest_start,
                dest_end,
                to_dest_kind(dest.kind),
                source.delete_source,
            )?;
            Ok(RelocateStep::Applied {
                message: dest_done(dest.kind, source.delete_source),
                shift,
            })
        }
    }
}

struct DestAt {
    start: usize,
    end: usize,
    kind: MarkKind,
    number: usize,
}

fn store_dest(dest: DestAt) -> PendingBlock {
    match dest.kind {
        MarkKind::Before => PendingBlock::Before {
            line: dest.start,
            repeat: dest.number,
        },
        MarkKind::After => PendingBlock::After {
            line: dest.start,
            repeat: dest.number,
        },
        kind if dest.start != dest.end || kind.is_block_dest() => PendingBlock::DestSpan {
            start: dest.start,
            end: dest.end,
            kind: to_dest_kind(kind),
            repeat: dest.number,
        },
        kind => PendingBlock::DestLine {
            line: dest.start,
            kind: to_dest_kind(kind),
            repeat: dest.number,
        },
    }
}

fn to_dest_kind(kind: MarkKind) -> crate::document::DestKind {
    match kind {
        MarkKind::Overlay | MarkKind::BlockOverlay => crate::document::DestKind::Overlay,
        MarkKind::OverlayReplace | MarkKind::BlockOverlayReplace => {
            crate::document::DestKind::OverlayReplace
        }
        MarkKind::Here | MarkKind::BlockHere => crate::document::DestKind::Here,
        MarkKind::AfterEach => crate::document::DestKind::AfterEach,
        MarkKind::BeforeEach => crate::document::DestKind::BeforeEach,
        _ => crate::document::DestKind::Swap,
    }
}

fn from_dest_kind(kind: crate::document::DestKind, block: bool) -> MarkKind {
    match (kind, block) {
        (crate::document::DestKind::Overlay, false) => MarkKind::Overlay,
        (crate::document::DestKind::Overlay, true) => MarkKind::BlockOverlay,
        (crate::document::DestKind::OverlayReplace, false) => MarkKind::OverlayReplace,
        (crate::document::DestKind::OverlayReplace, true) => MarkKind::BlockOverlayReplace,
        (crate::document::DestKind::Here, false) => MarkKind::Here,
        (crate::document::DestKind::Here, true) => MarkKind::BlockHere,
        (crate::document::DestKind::AfterEach, _) => MarkKind::AfterEach,
        (crate::document::DestKind::BeforeEach, _) => MarkKind::BeforeEach,
        (crate::document::DestKind::Swap, false) => MarkKind::Swap,
        (crate::document::DestKind::Swap, true) => MarkKind::BlockSwap,
    }
}

fn dest_label(kind: MarkKind) -> &'static str {
    crate::document::dest_name(to_dest_kind(kind), kind.is_block_dest())
}

fn dest_done(kind: MarkKind, moved: bool) -> String {
    match kind {
        MarkKind::Overlay | MarkKind::BlockOverlay => "Overlayed".to_string(),
        MarkKind::OverlayReplace | MarkKind::BlockOverlayReplace => "Overlayed".to_string(),
        MarkKind::Swap | MarkKind::BlockSwap => "Swapped".to_string(),
        MarkKind::Here | MarkKind::BlockHere if moved => "Moved".to_string(),
        MarkKind::AfterEach | MarkKind::BeforeEach if moved => "Moved".to_string(),
        _ if moved => "Moved".to_string(),
        _ => "Copied".to_string(),
    }
}

struct SourceRange {
    start: usize,
    end: usize,
    lines: usize,
    factor: usize,
    block: bool,
    delete_source: bool,
}

fn one_source(doc: &Document, mark: &Mark) -> EditResult<SourceRange> {
    let lines = mark.number.max(1);
    let end = doc.span_end(mark.line, lines)?;
    Ok(SourceRange {
        start: mark.line,
        end,
        lines,
        factor: 0,
        block: false,
        delete_source: mark.kind.is_move(),
    })
}

fn block_source(doc: &Document, left: &Mark, right: &Mark) -> EditResult<SourceRange> {
    let (start, end) = expand_block(doc, left.line, right.line)?;
    Ok(SourceRange {
        start,
        end,
        lines: 0,
        factor: merge_repeat(left.number, right.number)?,
        block: true,
        delete_source: left.kind.is_move(),
    })
}

fn merge_repeat(left: usize, right: usize) -> EditResult<usize> {
    match (left, right) {
        (0, value) | (value, 0) => Ok(value),
        (left, right) if left == right => Ok(left),
        _ => Err(EditError::new("Different numeric operands on 1 range")),
    }
}

fn open_block(mark: &Mark) -> PendingBlock {
    if mark.kind.is_move() {
        PendingBlock::MoveOpen {
            line: mark.line,
            repeat: mark.number,
        }
    } else {
        PendingBlock::CopyOpen {
            line: mark.line,
            repeat: mark.number,
        }
    }
}

fn source_pending(source: SourceRange) -> PendingBlock {
    if source.delete_source {
        if source.block {
            PendingBlock::MoveSpan {
                start: source.start,
                end: source.end,
                repeat: source.factor,
            }
        } else {
            PendingBlock::MoveLine {
                line: source.start,
                lines: source.lines,
            }
        }
    } else if source.block {
        PendingBlock::CopySpan {
            start: source.start,
            end: source.end,
            repeat: source.factor,
        }
    } else {
        PendingBlock::CopyLine {
            line: source.start,
            lines: source.lines,
        }
    }
}

fn pending_status(pending: PendingBlock) -> String {
    match pending {
        PendingBlock::CopyOpen { .. } | PendingBlock::MoveOpen { .. } => {
            "Pending command(s)".to_string()
        }
        PendingBlock::After { .. }
        | PendingBlock::Before { .. }
        | PendingBlock::DestLine { .. }
        | PendingBlock::DestSpan { .. } => "Pending source line command".to_string(),
        PendingBlock::DestOpen { .. } => "Pending command(s)".to_string(),
        _ => "Pending destination line command".to_string(),
    }
}

fn cancels(pending: PendingBlock, mark: &Mark) -> bool {
    match pending {
        PendingBlock::CopyLine { line, lines } => {
            mark.kind == MarkKind::Copy && mark.line == line && mark.number == lines
        }
        PendingBlock::MoveLine { line, lines } => {
            mark.kind == MarkKind::Move && mark.line == line && mark.number == lines
        }
        PendingBlock::CopyOpen { line, repeat } => {
            mark.kind == MarkKind::BlockCopy && mark.line == line && mark.number == repeat
        }
        PendingBlock::MoveOpen { line, repeat } => {
            mark.kind == MarkKind::BlockMove && mark.line == line && mark.number == repeat
        }
        PendingBlock::CopySpan { start, end, repeat } => {
            mark.kind == MarkKind::BlockCopy
                && (mark.line == start || mark.line == end)
                && mark.number == repeat
        }
        PendingBlock::MoveSpan { start, end, repeat } => {
            mark.kind == MarkKind::BlockMove
                && (mark.line == start || mark.line == end)
                && mark.number == repeat
        }
        PendingBlock::After { line, repeat } => {
            mark.kind == MarkKind::After && mark.line == line && mark.number == repeat
        }
        PendingBlock::Before { line, repeat } => {
            mark.kind == MarkKind::Before && mark.line == line && mark.number == repeat
        }
        PendingBlock::DestLine { line, kind, repeat } => {
            mark.kind == from_dest_kind(kind, false) && mark.line == line && mark.number == repeat
        }
        PendingBlock::DestOpen { line, kind, repeat } => {
            mark.kind == from_dest_kind(kind, true) && mark.line == line && mark.number == repeat
        }
        PendingBlock::DestSpan {
            start,
            end,
            kind,
            repeat,
        } => {
            mark.kind == from_dest_kind(kind, true)
                && (mark.line == start || mark.line == end)
                && mark.number == repeat
        }
        _ => false,
    }
}

fn pending_marks(pending: PendingBlock) -> Vec<Mark> {
    let mark = |line, kind, number| Mark {
        line,
        kind,
        number,
        user: false,
    };
    match pending {
        PendingBlock::CopyLine { line, lines } => vec![mark(line, MarkKind::Copy, lines)],
        PendingBlock::MoveLine { line, lines } => vec![mark(line, MarkKind::Move, lines)],
        PendingBlock::CopyOpen { line, repeat } => vec![mark(line, MarkKind::BlockCopy, repeat)],
        PendingBlock::MoveOpen { line, repeat } => vec![mark(line, MarkKind::BlockMove, repeat)],
        PendingBlock::CopySpan { start, end, repeat } => vec![
            mark(start, MarkKind::BlockCopy, repeat),
            mark(end, MarkKind::BlockCopy, repeat),
        ],
        PendingBlock::MoveSpan { start, end, repeat } => vec![
            mark(start, MarkKind::BlockMove, repeat),
            mark(end, MarkKind::BlockMove, repeat),
        ],
        PendingBlock::After { line, repeat } => vec![mark(line, MarkKind::After, repeat)],
        PendingBlock::Before { line, repeat } => vec![mark(line, MarkKind::Before, repeat)],
        PendingBlock::DestLine { line, kind, repeat } => {
            vec![mark(line, from_dest_kind(kind, false), repeat)]
        }
        PendingBlock::DestOpen { line, kind, repeat } => {
            vec![mark(line, from_dest_kind(kind, true), repeat)]
        }
        PendingBlock::DestSpan {
            start,
            end,
            kind,
            repeat,
        } => vec![
            mark(start, from_dest_kind(kind, true), repeat),
            mark(end, from_dest_kind(kind, true), repeat),
        ],
        _ => Vec::new(),
    }
}

fn mark_kind(item: ParsedLine) -> MarkKind {
    match (item.kind, item.block) {
        (LineKind::Copy, false) => MarkKind::Copy,
        (LineKind::Copy, true) => MarkKind::BlockCopy,
        (LineKind::Move, false) => MarkKind::Move,
        (LineKind::Move, true) => MarkKind::BlockMove,
        (LineKind::Before, _) => MarkKind::Before,
        (LineKind::Overlay, false) => MarkKind::Overlay,
        (LineKind::Overlay, true) => MarkKind::BlockOverlay,
        (LineKind::OverlayReplace, false) => MarkKind::OverlayReplace,
        (LineKind::OverlayReplace, true) => MarkKind::BlockOverlayReplace,
        (LineKind::Here, false) => MarkKind::Here,
        (LineKind::Here, true) => MarkKind::BlockHere,
        (LineKind::AfterEach, _) => MarkKind::AfterEach,
        (LineKind::BeforeEach, _) => MarkKind::BeforeEach,
        (LineKind::Swap, false) => MarkKind::Swap,
        (LineKind::Swap, true) => MarkKind::BlockSwap,
        _ => MarkKind::After,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Delete,
    Insert,
    Exclude,
    First,
    Last,
    Show,
    Copy,
    Move,
    After,
    Before,
    Repeat,
    Upper,
    Lower,
    ColLeft,
    ColRight,
    DataLeft,
    DataRight,
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
    TabsLine,
    Bnds,
    Mask,
    Word,
    Mark,
    Text,
    Overlay,
    OverlayReplace,
    Here,
    AfterEach,
    BeforeEach,
    Swap,
}

impl LineKind {
    fn is_relocate(self) -> bool {
        matches!(
            self,
            LineKind::Copy
                | LineKind::Move
                | LineKind::After
                | LineKind::Before
                | LineKind::Overlay
                | LineKind::OverlayReplace
                | LineKind::Here
                | LineKind::AfterEach
                | LineKind::BeforeEach
                | LineKind::Swap
        )
    }
}

#[derive(Clone, Copy)]
struct ParsedLine {
    kind: LineKind,
    block: bool,
    repeat: usize,
}

fn parse_line_command(command: &str) -> EditResult<ParsedLine> {
    let command = command.trim().to_ascii_uppercase();
    if command.is_empty() {
        return Err(EditError::new("Missing line command"));
    }
    let split_at = command
        .chars()
        .position(|ch| ch.is_ascii_digit())
        .unwrap_or(command.len());
    let letters = &command[..split_at];
    let digits = &command[split_at..];
    if digits.chars().any(|ch| !ch.is_ascii_digit()) {
        return Err(EditError::new(format!("Unknown line command '{command}'")));
    }
    let (kind, block) = match letters {
        "D" => (LineKind::Delete, false),
        "DD" => (LineKind::Delete, true),
        "I" | "N" => (LineKind::Insert, false),
        "X" => (LineKind::Exclude, false),
        "XX" => (LineKind::Exclude, true),
        "F" => (LineKind::First, false),
        "L" => (LineKind::Last, false),
        "S" => (LineKind::Show, false),
        "C" => (LineKind::Copy, false),
        "CC" => (LineKind::Copy, true),
        "M" => (LineKind::Move, false),
        "MM" => (LineKind::Move, true),
        "A" => (LineKind::After, false),
        "B" => (LineKind::Before, false),
        "R" => (LineKind::Repeat, false),
        "RR" => (LineKind::Repeat, true),
        "UC" => (LineKind::Upper, false),
        "UCC" => (LineKind::Upper, true),
        "LC" => (LineKind::Lower, false),
        "LCC" => (LineKind::Lower, true),
        "(" => (LineKind::ColLeft, false),
        "((" => (LineKind::ColLeft, true),
        ")" => (LineKind::ColRight, false),
        "))" => (LineKind::ColRight, true),
        "<" => (LineKind::DataLeft, false),
        "<<" => (LineKind::DataLeft, true),
        ">" => (LineKind::DataRight, false),
        ">>" => (LineKind::DataRight, true),
        "J" => (LineKind::Join, false),
        "JJ" => (LineKind::Join, true),
        "G" => (LineKind::Glue, false),
        "GG" => (LineKind::Glue, true),
        "TJ" => (LineKind::TextJoin, false),
        "TJJ" => (LineKind::TextJoin, true),
        "TG" => (LineKind::TextGlue, false),
        "TGG" => (LineKind::TextGlue, true),
        "SC" => (LineKind::Sentence, false),
        "SCC" => (LineKind::Sentence, true),
        "TC" => (LineKind::Title, false),
        "TCC" => (LineKind::Title, true),
        "TR" => (LineKind::Trim, false),
        "TRR" => (LineKind::Trim, true),
        "PL" => (LineKind::Pad, false),
        "PLL" => (LineKind::Pad, true),
        "TL" => (LineKind::Trunc, false),
        "TLL" => (LineKind::Trunc, true),
        "[" => (LineKind::IndentLeft, false),
        "[[" => (LineKind::IndentLeft, true),
        "]" => (LineKind::IndentRight, false),
        "]]" => (LineKind::IndentRight, true),
        "TS" | "TB" | "TBB" => (LineKind::Split, false),
        "TF" => (LineKind::Flow, false),
        "TFF" => (LineKind::Flow, true),
        "TM" => (LineKind::Margin, false),
        "TMM" => (LineKind::Margin, true),
        "NOTE" => (LineKind::Note, false),
        "MN" => (LineKind::MakeNote, false),
        "MNN" => (LineKind::MakeNote, true),
        "MD" => (LineKind::MakeData, false),
        "MDD" => (LineKind::MakeData, true),
        "U" => (LineKind::User, false),
        "UU" => (LineKind::User, true),
        "V" => (LineKind::Revert, false),
        "VV" => (LineKind::Revert, true),
        "TX" => (LineKind::ToggleX, false),
        "TXX" => (LineKind::ToggleX, true),
        "TU" => (LineKind::ToggleUser, false),
        "TUU" => (LineKind::ToggleUser, true),
        "COLS" | "COL" => (LineKind::Cols, false),
        "TABS" | "TAB" => (LineKind::TabsLine, false),
        "BNDS" | "BND" | "BOUNDS" | "BOU" => (LineKind::Bnds, false),
        "MASK" => (LineKind::Mask, false),
        "WORD" | "DLM" => (LineKind::Word, false),
        "MARK" => (LineKind::Mark, false),
        "T" | "TT" => (LineKind::Text, false),
        "O" => (LineKind::Overlay, false),
        "OO" => (LineKind::Overlay, true),
        "OR" => (LineKind::OverlayReplace, false),
        "ORR" => (LineKind::OverlayReplace, true),
        "H" => (LineKind::Here, false),
        "HH" => (LineKind::Here, true),
        "AA" => (LineKind::AfterEach, true),
        "BB" => (LineKind::BeforeEach, true),
        "W" => (LineKind::Swap, false),
        "WW" => (LineKind::Swap, true),
        _ => return Err(EditError::new(format!("Unknown line command '{command}'"))),
    };
    // `C3` and `UC3` count lines. `R3` repeats the line. `)3` shifts 3
    // columns, and a missing shift count means 2. `A2`, `CC2`, `RR2`, and a
    // shift block use 0 for "not specified".
    let shift = matches!(
        kind,
        LineKind::ColLeft | LineKind::ColRight | LineKind::DataLeft | LineKind::DataRight
    );
    let dest = matches!(
        kind,
        LineKind::Overlay
            | LineKind::OverlayReplace
            | LineKind::Here
            | LineKind::AfterEach
            | LineKind::BeforeEach
            | LineKind::Swap
    );
    let width = matches!(
        kind,
        LineKind::Split | LineKind::Pad | LineKind::Trunc | LineKind::Flow | LineKind::Margin
    );
    let factor = matches!(
        (kind, block),
        (LineKind::After | LineKind::Before, _)
            | (LineKind::Copy | LineKind::Move | LineKind::Repeat, true)
    ) || (block && shift)
        || dest;
    let repeat = if digits.is_empty() {
        if factor || width {
            0
        } else if shift {
            2
        } else {
            1
        }
    } else {
        digits
            .parse::<usize>()
            .map_err(|_| EditError::new("Repeat count is invalid"))?
    };
    if repeat == 0 && !factor && !width {
        return Err(EditError::new("Repeat count is zero"));
    }
    let counted_block = matches!(
        kind,
        LineKind::Copy
            | LineKind::Move
            | LineKind::Repeat
            | LineKind::ColLeft
            | LineKind::ColRight
            | LineKind::DataLeft
            | LineKind::DataRight
            | LineKind::Pad
            | LineKind::Trunc
            | LineKind::Flow
            | LineKind::Margin
            | LineKind::IndentLeft
            | LineKind::IndentRight
            | LineKind::Overlay
            | LineKind::OverlayReplace
            | LineKind::Here
            | LineKind::AfterEach
            | LineKind::BeforeEach
            | LineKind::Swap
    );
    if block && !digits.is_empty() && !counted_block {
        return Err(EditError::new("A block line command does not take a count"));
    }
    Ok(ParsedLine {
        kind,
        block,
        repeat,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Next,
    Prev,
    First,
    Last,
    All,
}

struct Needle {
    text: String,
    case_sensitive: bool,
    len: usize,
    picture: bool,
    bound: Bound,
    lines: LineFilter,
    /// When set, a match must start in this 1-based column.
    column: Option<usize>,
    /// Inclusive end column. Absent with `column` means the string must start there.
    column_end: Option<usize>,
    line_start: Option<usize>,
    line_end: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Chars,
    Prefix,
    Suffix,
    Word,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineFilter {
    Any,
    Excluded,
    Shown,
}

struct Hit {
    line: usize,
    col: usize,
    len: usize,
}

fn parse_search(doc: &Document, rest: &str) -> EditResult<(Needle, Which, String)> {
    let pieces = scan_change_pieces(rest)?;
    let (operand, keywords) = assign_search_operands(pieces)?;
    if operand.text.is_empty() && !operand.picture {
        return Err(EditError::new("Search string is empty"));
    }
    let keyword_line = keywords.join(" ");
    let qualifiers = without_which(&keyword_line);
    let mut which = Which::Next;
    let mut needle = needle_from(operand);
    apply_qualifiers(doc, &mut needle, &mut which, &keyword_line)?;
    Ok((needle, which, qualifiers))
}

/// One search string, and the other words are parameters in any order.
/// `F ALL .a .b cat` is the same search as `F cat ALL .a .b`. A lone keyword
/// is the string, so `F ALL` looks for the word ALL. A single letter such as
/// `a` stays the string in `EXCLUDE a ALL`.
fn assign_search_operands(pieces: Vec<ChangePiece>) -> EditResult<(Operand, Vec<String>)> {
    if pieces.is_empty() {
        return Err(EditError::new("Missing string"));
    }
    let mut words = Vec::new();
    let mut softs = Vec::new();
    let mut hards = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        match piece {
            ChangePiece::Forced(_) => words.push(index),
            ChangePiece::Bare(word) => match search_role(word) {
                SearchRole::Word => words.push(index),
                SearchRole::Soft => softs.push(index),
                SearchRole::Hard => hards.push(index),
            },
        }
    }
    let string_at =
        if words.len() == 1 {
            words[0]
        } else if words.len() > 1 {
            return Err(EditError::new("Too many operands"));
        } else if softs.len() == 1 {
            softs[0]
        } else if softs.len() > 1 {
            return Err(EditError::new("Too many operands"));
        } else if hards.len() == 1 {
            hards[0]
        } else if let Some(column) = hards.iter().copied().find(
            |index| matches!(&pieces[*index], ChangePiece::Bare(word) if is_column_token(word)),
        ) {
            column
        } else {
            return Err(EditError::new("Search string is empty"));
        };
    let mut string = None;
    let mut keywords = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        if index == string_at {
            string = Some(match piece {
                ChangePiece::Forced(operand) => operand,
                ChangePiece::Bare(word) => Operand {
                    text: word,
                    case_sensitive: false,
                    picture: false,
                },
            });
        } else if let ChangePiece::Bare(word) = piece {
            keywords.push(word);
        } else {
            return Err(EditError::new("Too many operands"));
        }
    }
    Ok((
        string.ok_or_else(|| EditError::new("Search string is empty"))?,
        keywords,
    ))
}

enum SearchRole {
    Word,
    /// A one-letter parameter. It is the search string when no ordinary word is present.
    Soft,
    Hard,
}

fn search_role(word: &str) -> SearchRole {
    let upper = word.to_ascii_uppercase();
    if matches!(upper.as_str(), "N" | "P" | "F" | "L" | "A" | "X") {
        return SearchRole::Soft;
    }
    if matches!(
        upper.as_str(),
        "NEXT"
            | "PREV"
            | "PREVIOUS"
            | "FIRST"
            | "LAST"
            | "ALL"
            | "CHARS"
            | "CHAR"
            | "PREFIX"
            | "SUFFIX"
            | "WORD"
            | "NX"
    ) || upper.starts_with('.')
        || is_column_token(&upper)
    {
        return SearchRole::Hard;
    }
    SearchRole::Word
}

fn is_column_token(word: &str) -> bool {
    !word.is_empty() && word != "0" && word.chars().all(|ch| ch.is_ascii_digit())
}

fn parse_change(doc: &Document, rest: &str) -> EditResult<(Needle, String, Which, String)> {
    let pieces = scan_change_pieces(rest)?;
    let (mut strings, keywords) = assign_change_operands(pieces)?;
    let new_op = strings
        .pop()
        .ok_or_else(|| EditError::new("Missing string"))?;
    let old = strings
        .pop()
        .ok_or_else(|| EditError::new("Missing string"))?;
    if old.text.is_empty() && !old.picture {
        return Err(EditError::new("Change string is empty"));
    }
    let keyword_line = keywords.join(" ");
    let qualifiers = without_which(&keyword_line);
    let mut which = Which::Next;
    let mut needle = needle_from(old);
    apply_qualifiers(doc, &mut needle, &mut which, &keyword_line)?;
    Ok((needle, new_op.text, which, qualifiers))
}

/// A quoted or typed literal is always a string. A bare keyword is a parameter
/// when two strings remain among the other words, so `C ALL a b`, `C a ALL b`,
/// and `C a b ALL` are the same change.
enum ChangePiece {
    Forced(Operand),
    Bare(String),
}

fn scan_change_pieces(input: &str) -> EditResult<Vec<ChangePiece>> {
    let mut pieces = Vec::new();
    let mut rest = input.trim_start();
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        let forced = bytes[0] == b'\''
            || bytes[0] == b'"'
            || (bytes.len() >= 2
                && matches!(bytes[0].to_ascii_uppercase(), b'C' | b'T' | b'P' | b'X')
                && (bytes[1] == b'\'' || bytes[1] == b'"'));
        let (operand, next) = take_operand(rest)?;
        if forced {
            pieces.push(ChangePiece::Forced(operand));
        } else {
            pieces.push(ChangePiece::Bare(operand.text));
        }
        rest = next;
    }
    Ok(pieces)
}

fn assign_change_operands(pieces: Vec<ChangePiece>) -> EditResult<(Vec<Operand>, Vec<String>)> {
    let mut need = 2usize;
    let mut strings = Vec::new();
    let mut keywords = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let later = pieces.len() - index - 1;
        match piece {
            ChangePiece::Forced(operand) => {
                strings.push(operand.clone());
                need = need.saturating_sub(1);
            }
            ChangePiece::Bare(word) if is_change_keyword(word) && (need == 0 || later >= need) => {
                keywords.push(word.clone());
            }
            ChangePiece::Bare(word) => {
                strings.push(Operand {
                    text: word.clone(),
                    case_sensitive: false,
                    picture: false,
                });
                need = need.saturating_sub(1);
            }
        }
    }
    if strings.len() < 2 {
        return Err(EditError::new("Missing string"));
    }
    if strings.len() > 2 {
        return Err(EditError::new("Too many operands"));
    }
    Ok((strings, keywords))
}

fn is_change_keyword(word: &str) -> bool {
    if which_token(word).is_some() {
        return true;
    }
    let upper = word.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CHARS" | "CHAR" | "PREFIX" | "SUFFIX" | "WORD" | "NX" | "X"
    ) || upper.starts_with('.')
        || (!upper.is_empty() && upper != "0" && upper.chars().all(|c| c.is_ascii_digit()))
}

fn needle_from(operand: Operand) -> Needle {
    let len = operand.text.chars().count();
    Needle {
        text: operand.text,
        case_sensitive: operand.case_sensitive,
        len,
        picture: operand.picture,
        bound: Bound::Chars,
        lines: LineFilter::Any,
        column: None,
        column_end: None,
        line_start: None,
        line_end: None,
    }
}

fn apply_qualifiers(
    doc: &Document,
    needle: &mut Needle,
    which: &mut Which,
    rest: &str,
) -> EditResult<()> {
    let mut labels = Vec::new();
    let mut columns = Vec::new();
    let mut saw_which = false;
    let mut saw_bound = false;
    for token in rest.split_whitespace() {
        if let Some(next) = which_token(token) {
            if saw_which {
                return Err(EditError::new("Inconsistent parameters"));
            }
            saw_which = true;
            *which = next;
            continue;
        }
        let upper = token.to_ascii_uppercase();
        if upper.starts_with('.') {
            labels.push(upper);
            continue;
        }
        match upper.as_str() {
            "CHARS" | "CHAR" => {
                if saw_bound {
                    return Err(EditError::new("Inconsistent parameters"));
                }
                saw_bound = true;
                needle.bound = Bound::Chars;
            }
            "PREFIX" => {
                if saw_bound {
                    return Err(EditError::new("Inconsistent parameters"));
                }
                saw_bound = true;
                needle.bound = Bound::Prefix;
            }
            "SUFFIX" => {
                if saw_bound {
                    return Err(EditError::new("Inconsistent parameters"));
                }
                saw_bound = true;
                needle.bound = Bound::Suffix;
            }
            "WORD" => {
                if saw_bound {
                    return Err(EditError::new("Inconsistent parameters"));
                }
                saw_bound = true;
                needle.bound = Bound::Word;
            }
            "NX" => {
                if needle.lines != LineFilter::Any {
                    return Err(EditError::new("Inconsistent parameters"));
                }
                needle.lines = LineFilter::Shown;
            }
            "X" => {
                if needle.lines != LineFilter::Any {
                    return Err(EditError::new("Inconsistent parameters"));
                }
                needle.lines = LineFilter::Excluded;
            }
            _ if upper.chars().all(|c| c.is_ascii_digit()) && !upper.is_empty() => {
                let column: usize = token
                    .parse()
                    .map_err(|_| EditError::new("Column is not a number"))?;
                if column == 0 {
                    return Err(EditError::new("Column is not a number"));
                }
                columns.push(column);
            }
            other => return Err(EditError::new(format!("Unknown keyword '{other}'"))),
        }
    }
    if labels.len() == 1 || labels.len() > 2 {
        return Err(EditError::new("Two labels are required"));
    }
    if labels.len() == 2 {
        let start = doc.resolve_line(&labels[0])?;
        let end = doc.resolve_line(&labels[1])?;
        needle.line_start = Some(start.min(end));
        needle.line_end = Some(start.max(end));
    }
    if columns.len() > 2 {
        return Err(EditError::new("Too many columns"));
    }
    if let Some(start) = columns.first().copied() {
        needle.column = Some(start);
    }
    if columns.len() == 2 {
        let end = columns[1];
        if end < columns[0] {
            return Err(EditError::new("The end column is before the start column"));
        }
        needle.column_end = Some(end);
    }
    Ok(())
}

fn which_token(token: &str) -> Option<Which> {
    match token.to_ascii_uppercase().as_str() {
        "NEXT" | "N" => Some(Which::Next),
        "PREV" | "PREVIOUS" | "P" => Some(Which::Prev),
        "FIRST" | "F" => Some(Which::First),
        "LAST" | "L" => Some(Which::Last),
        "ALL" | "A" => Some(Which::All),
        _ => None,
    }
}

fn without_which(rest: &str) -> String {
    rest.split_whitespace()
        .filter(|token| which_token(token).is_none())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone)]
struct Operand {
    text: String,
    case_sensitive: bool,
    picture: bool,
}

fn take_operand(input: &str) -> EditResult<(Operand, &str)> {
    let s = input.trim_start();
    if s.is_empty() {
        return Err(EditError::new("Missing string"));
    }
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && (bytes[1] == b'\'' || bytes[1] == b'"') {
        let kind = bytes[0].to_ascii_uppercase();
        if matches!(kind, b'C' | b'T' | b'P' | b'X') {
            let (text, rest) = take_quoted(&s[1..])?;
            return operand_kind(kind, text, rest);
        }
    }
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        let (text, rest) = take_quoted(s)?;
        return Ok((
            Operand {
                text,
                case_sensitive: false,
                picture: false,
            },
            rest,
        ));
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Ok((
        Operand {
            text: s[..end].to_string(),
            case_sensitive: false,
            picture: false,
        },
        s[end..].trim_start(),
    ))
}

fn operand_kind(kind: u8, text: String, rest: &str) -> EditResult<(Operand, &str)> {
    match kind {
        b'C' => Ok((
            Operand {
                text,
                case_sensitive: true,
                picture: false,
            },
            rest,
        )),
        b'T' => Ok((
            Operand {
                text,
                case_sensitive: false,
                picture: false,
            },
            rest,
        )),
        b'P' => {
            if text.is_empty() {
                return Err(EditError::new("Search string is empty"));
            }
            Ok((
                Operand {
                    text,
                    case_sensitive: true,
                    picture: true,
                },
                rest,
            ))
        }
        b'X' => {
            let decoded = decode_hex(&text)?;
            Ok((
                Operand {
                    text: decoded,
                    case_sensitive: true,
                    picture: false,
                },
                rest,
            ))
        }
        _ => Err(EditError::new("Missing string")),
    }
}

fn decode_hex(text: &str) -> EditResult<String> {
    let hex: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if hex.is_empty() || hex.len() % 2 != 0 {
        return Err(EditError::new("Invalid hex string"));
    }
    let mut out = String::new();
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let pair = std::str::from_utf8(&bytes[i..i + 2]).unwrap_or("");
        let value =
            u8::from_str_radix(pair, 16).map_err(|_| EditError::new("Invalid hex string"))?;
        out.push(value as char);
        i += 2;
    }
    Ok(out)
}

fn take_quoted(input: &str) -> EditResult<(String, &str)> {
    let mut chars = input.chars();
    let quote = chars
        .next()
        .ok_or_else(|| EditError::new("Missing quote"))?;
    if quote != '\'' && quote != '"' {
        return Err(EditError::new("Missing quote"));
    }
    let mut out = String::new();
    let mut consumed = quote.len_utf8();
    let rest_chars: Vec<char> = chars.collect();
    let mut i = 0;
    while i < rest_chars.len() {
        let c = rest_chars[i];
        consumed += c.len_utf8();
        i += 1;
        if c == quote {
            if rest_chars.get(i) == Some(&quote) {
                consumed += quote.len_utf8();
                i += 1;
                out.push(quote);
            } else {
                return Ok((out, input[consumed..].trim_start()));
            }
        } else {
            out.push(c);
        }
    }
    Err(EditError::new("Unclosed quote"))
}

/// `skip_current` is set for FIND, so a second FIND moves off the match the
/// cursor is already sitting on. CHANGE leaves it false and edits that match.
/// `screen_line` is the first line on screen. NEXT starts at its first column.
fn seek(
    doc: &Document,
    needle: &Needle,
    which: Which,
    skip_current: bool,
    screen_line: Option<usize>,
) -> EditResult<Hit> {
    let (cursor_line, cursor_col) = doc.cursor();
    let from_screen = screen_line.filter(|_| which == Which::Next);
    let (start_line, start_col) = match from_screen {
        Some(line) => (line, 1),
        None => (cursor_line, cursor_col),
    };
    let skip = if from_screen.is_none() && skip_current && doc.cursor_on_find() {
        Some(cursor_col)
    } else {
        None
    };
    let hit = match which {
        Which::Next => find_forward(doc, needle, start_line, start_col, skip),
        Which::Prev => find_backward(doc, needle, cursor_line, cursor_col, skip),
        Which::First => find_forward(doc, needle, 2, 1, None),
        Which::Last => find_backward(doc, needle, doc.last_line(), usize::MAX, None),
        Which::All => None,
    };
    hit.ok_or_else(|| not_found(needle))
}

fn find_forward(
    doc: &Document,
    needle: &Needle,
    start_line: usize,
    start_col: usize,
    skip: Option<usize>,
) -> Option<Hit> {
    let mut line = start_line.max(1);
    let mut min_col = start_col;
    while line <= doc.last_line() {
        for col in match_columns(doc, line, needle) {
            if line == start_line && col < min_col {
                continue;
            }
            if line == start_line && skip == Some(col) {
                continue;
            }
            return Some(Hit {
                line,
                col,
                len: needle.len,
            });
        }
        line += 1;
        min_col = 1;
    }
    None
}

fn find_backward(
    doc: &Document,
    needle: &Needle,
    start_line: usize,
    start_col: usize,
    skip: Option<usize>,
) -> Option<Hit> {
    let mut line = start_line.min(doc.last_line());
    let mut limit = Some(start_col);
    while line >= 1 {
        let mut chosen = None;
        for col in match_columns(doc, line, needle) {
            if let Some(max_col) = limit {
                if col > max_col {
                    continue;
                }
            }
            if line == start_line && skip == Some(col) {
                continue;
            }
            chosen = Some(col);
        }
        if let Some(col) = chosen {
            return Some(Hit {
                line,
                col,
                len: needle.len,
            });
        }
        if line == 1 {
            break;
        }
        line -= 1;
        limit = None;
    }
    None
}

fn match_columns(doc: &Document, line: usize, needle: &Needle) -> Vec<usize> {
    if let Some(start) = needle.line_start {
        if line < start {
            return Vec::new();
        }
    }
    if let Some(end) = needle.line_end {
        if line > end {
            return Vec::new();
        }
    }
    let Ok(row) = doc.line(line) else {
        return Vec::new();
    };
    if !row.is_data() && !row.is_note() {
        return Vec::new();
    }
    match needle.lines {
        LineFilter::Excluded if !row.is_invisible() => return Vec::new(),
        LineFilter::Shown if row.is_invisible() => return Vec::new(),
        _ => {}
    }
    let chars = row.chars();
    let folded: Vec<char> = needle
        .text
        .chars()
        .map(|c| fold(c, needle.case_sensitive))
        .collect();
    if folded.is_empty() || folded.len() > chars.len() {
        return Vec::new();
    }
    let bounds_left = doc.profile().bounds_left.max(1);
    let bounds_right = if doc.profile().bounds_right == 0 {
        chars.len()
    } else {
        doc.profile().bounds_right.min(chars.len())
    };
    let (left, right, pinned) = if let Some(column) = needle.column {
        if let Some(end) = needle.column_end {
            let left = column.max(bounds_left);
            let right = end.min(bounds_right);
            (left.saturating_sub(1), right, false)
        } else {
            (
                column.saturating_sub(1),
                column.saturating_sub(1) + folded.len(),
                true,
            )
        }
    } else {
        (bounds_left - 1, bounds_right, false)
    };
    if left >= right || folded.len() > right.saturating_sub(left) {
        return Vec::new();
    }
    let mut hits = Vec::new();
    let mut i = left;
    let last = right.saturating_sub(folded.len());
    while i <= last && i + folded.len() <= chars.len() {
        let matched = if needle.picture {
            needle
                .text
                .chars()
                .zip(chars[i..i + folded.len()].iter())
                .all(|(pattern, have)| picture_matches(pattern, *have))
        } else {
            chars[i..i + folded.len()]
                .iter()
                .zip(&folded)
                .all(|(have, want)| fold(*have, needle.case_sensitive) == *want)
        };
        let column = i + 1;
        if matched
            && boundary_ok(&chars, i, folded.len(), needle.bound)
            && (!pinned || column == needle.column.unwrap_or(column))
        {
            hits.push(column);
            i += folded.len();
        } else {
            i += 1;
        }
        if pinned {
            break;
        }
    }
    hits
}

fn boundary_ok(chars: &[char], start: usize, len: usize, bound: Bound) -> bool {
    let end = start + len;
    let left = start == 0 || !is_word_char(chars[start - 1]);
    let right = end >= chars.len() || !is_word_char(chars[end]);
    match bound {
        Bound::Chars => true,
        Bound::Prefix => left,
        Bound::Suffix => right,
        Bound::Word => left && right,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

fn picture_matches(pattern: char, have: char) -> bool {
    match pattern {
        '=' => true,
        '¬' | '^' | '~' => !have.is_whitespace(),
        '.' => have.is_control(),
        '#' => have.is_ascii_digit(),
        '-' => !have.is_ascii_digit(),
        '@' => have.is_ascii_alphabetic(),
        '<' => have.is_ascii_lowercase(),
        '>' => have.is_ascii_uppercase(),
        '$' => !have.is_ascii_alphanumeric() && !have.is_whitespace(),
        other => have == other,
    }
}

fn fold(c: char, case_sensitive: bool) -> char {
    if case_sensitive {
        c
    } else {
        c.to_ascii_lowercase()
    }
}

fn not_found(needle: &Needle) -> EditError {
    EditError::new(format!("'{}' not found", needle.text))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn split_statements(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in input.chars() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            current.push(c);
        } else if c == ';' {
            if !current.trim().is_empty() {
                out.push(current.trim().to_string());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn split_verb(input: &str) -> (&str, &str) {
    let input = input.trim();
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    (input[..end].trim(), input[end..].trim())
}

fn split_token(input: &str) -> EditResult<(&str, &str)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(EditError::new("Missing operand"));
    }
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    let rest = input[end..].trim();
    if rest.is_empty() {
        return Err(EditError::new("Missing operand"));
    }
    Ok((&input[..end], rest))
}

fn split_token_optional(input: &str) -> EditResult<(&str, &str)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(EditError::new("Missing operand"));
    }
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    Ok((&input[..end], input[end..].trim()))
}

fn unwrap_quotes(text: &str) -> String {
    let text = text.trim();
    if text.len() >= 2 {
        let bytes = text.as_bytes();
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return text[1..text.len() - 1].to_string();
        }
    }
    text.to_string()
}

pub fn help_text() -> String {
    "\
FIND string [NEXT|ALL|FIRST|LAST|PREV] [CHARS|PREFIX|SUFFIX|WORD] [X|NX] [cols] [.lab1 .lab2]
CHANGE old new  NEXT|ALL|FIRST|LAST|PREV  CHARS|PREFIX|SUFFIX|WORD  X|NX  cols  .lab1 .lab2
  The two strings are required. Every other parameter may appear in any order,
  including before or between the strings. ALL is a string only when quoting it
  or when two other strings are not present.
LOCATE line | L line | .label            .ZF .ZL .HERE jumps to a bookmark
:line command                             :5 D   :.ZL I3   :2 XX
  line commands: D Dn DD  I In N  X Xn XX  F Fn  L Ln  S
                 C Cn CC  M Mn MM  A An  B Bn  R Rn RR
                 UC UCn UCC  LC LCn LCC
                 ( (n ((  ) )n ))  < <n <<  > >n >>  [ ] [[ ]]
                 J JJ  G GG  TJ TJJ  TG TGG  SC SCC  TC TCC
                 TR TRR  PL PLn  TL TLn  TS  TF  TM
                 O OO  OR ORR  H HH  AA BB  W WW
                 NOTE  MN MNN  MD MDD  U UU  V VV  TX TXX  TU TUU
                 COLS  TABS  BNDS  MASK  WORD  MARK  T
TEXT line new text
LABEL line .name                          LABEL line   clears it
BOOKMARK [name|LIST] | BM                 mark the cursor line; type .name in the number column
TAG line :name
BOUNDS [left [right]]                     right 0 runs through end of line
RESET | RES [CHANGE|COMMAND|ERROR|EXCLUDED|FIND|HIDE|LABEL|SPECIAL|ALL] [.lab1 .lab2]
                                          bare RESET keeps labels and clears the rest
DELETE string [NEXT|PREV|FIRST|LAST|ALL]
EXCLUDE ALL | EXCLUDE string [NEXT|PREV|FIRST|LAST|ALL]    ALL alone hides every line
SORT [A|D]
TOP | BOTTOM | UP [n] | DOWN [n] | LEFT [n] | RIGHT [n]
RFIND [PREV] | RCHANGE | RLOC | NFIND | NEXCLUDE | NDELETE
FLIP | SHOW [string] | HIDE [string]
CUT [n|ALL] | CLIP [n|ALL] | PASTE        CUT copies C/CC lines, or every line when none are marked
COPY path | CREATE path | REPLACE path | SAVEAS path | RENAME path
CANCEL | CAN                                  close without saving
EXIT                                          save and close
JOIN [n] | SPLIT | APPEND text | PREPEND text | ALIGN [col] | COMPRESS
UC | LC | SC | TC | LINE n | HEX | COLS [ON|OFF]
RENUM [ON|OFF] [STD] [COBOL] [DISPLAY]     number every line from 100 by 100
SWAPBAR [ON|OFF]                         the last line lists every screen
SWAP [PREV|NEXT|LIST|name|n]             F4 is SWAP LIST; F9 is SWAP
START                                    open a new screen
UNDO | REDO
OPEN path | SAVE [path] | ENCODING [UTF8|ANSI]
TABS | TAB n
Commands stack with ;"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    fn loaded(text: &str) -> Session {
        let mut session = Session::new();
        session.current_mut().doc = Document::from_text(text);
        session
    }

    #[test]
    fn find_walks_then_change_and_undo() {
        let mut session = loaded("Alpha beta\nBETA tail\nalpha\n");
        let msg = execute(&mut session, "FIND beta").unwrap();
        assert!(msg.contains("line 1"), "{msg}");
        assert_eq!(session.current().doc.cursor(), (2, 7));
        assert_eq!(session.current().doc.find_spans(2), vec![(7, 4)]);

        execute(&mut session, "F beta").unwrap();
        assert_eq!(
            session
                .current()
                .doc
                .line(session.current().doc.cursor().0)
                .unwrap()
                .seq,
            2
        );

        execute(&mut session, "F C'beta' FIRST").unwrap();
        assert_eq!(session.current().doc.cursor().0, 2);

        let mut either = loaded("ALL loan\nloan ALL\n");
        let keyword_first = execute(&mut either, "F ALL loan").unwrap();
        let spans = (
            either.current().doc.find_spans(2),
            either.current().doc.find_spans(3),
        );
        let cursor = either.current().doc.cursor();
        let mut string_first = loaded("ALL loan\nloan ALL\n");
        let keyword_last = execute(&mut string_first, "F loan ALL").unwrap();
        assert_eq!(keyword_first, keyword_last);
        assert_eq!(string_first.current().doc.cursor(), cursor);
        assert_eq!(
            (
                string_first.current().doc.find_spans(2),
                string_first.current().doc.find_spans(3),
            ),
            spans
        );
        assert_eq!(spans.0, vec![(5, 4)]);
        assert_eq!(spans.1, vec![(1, 4)]);

        let mut word = loaded("see ALL here\n");
        let msg = execute(&mut word, "F ALL").unwrap();
        assert!(msg.contains("'ALL'"), "{msg}");

        execute(&mut session, "CHANGE beta BETA ALL").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "Alpha BETA");
        assert_eq!(session.current().doc.line_text(3).unwrap(), "BETA tail");
        assert_eq!(session.current().doc.line_text(4).unwrap(), "alpha");
        assert!(session
            .current()
            .doc
            .line(2)
            .unwrap()
            .flags
            .contains(crate::line::LineFlags::EQ_CHANGE));

        execute(&mut session, "UNDO").unwrap();
        assert_eq!(
            session.current().doc.to_text(),
            "Alpha beta\nBETA tail\nalpha\n"
        );
    }

    #[test]
    fn bounds_and_quotes() {
        let mut session = loaded("xxfind me\nfind\n");
        execute(&mut session, "BOUNDS 5 8").unwrap();
        let err = execute(&mut session, "FIND 'find' FIRST").unwrap_err();
        assert!(err.message.contains("not found"), "{}", err.message);
        execute(&mut session, "BOUNDS").unwrap();
        execute(&mut session, "FIND 'find me'").unwrap();
        assert_eq!(
            session
                .current()
                .doc
                .line(session.current().doc.cursor().0)
                .unwrap()
                .seq,
            1
        );
    }

    #[test]
    fn line_commands_delete_insert_exclude() {
        let mut session = loaded("one\ntwo\nthree\nfour\nfive\n");
        execute(&mut session, ":2 D").unwrap();
        assert_eq!(session.current().doc.to_text(), "one\nthree\nfour\nfive\n");

        execute(&mut session, ":.ZL I").unwrap();
        assert!(session
            .current()
            .doc
            .line(session.current().doc.cursor().0)
            .unwrap()
            .is_insert());
        execute(&mut session, "TEXT .ZL inserted").unwrap();
        assert!(session.current().doc.to_text().ends_with("inserted\n"));

        execute(&mut session, ":2 XX").unwrap();
        execute(&mut session, ":3 XX").unwrap();
        assert_eq!(
            session
                .current()
                .doc
                .visible_rows()
                .iter()
                .filter(|row| row.exclude_count.is_some())
                .count(),
            1
        );
        execute(&mut session, "RESET").unwrap();
        assert!(session
            .current()
            .doc
            .visible_rows()
            .iter()
            .all(|row| row.exclude_count.is_none()));
        execute(&mut session, ":2 X").unwrap();
        execute(&mut session, "RES").unwrap();
        assert!(session
            .current()
            .doc
            .visible_rows()
            .iter()
            .all(|row| row.exclude_count.is_none()));
    }

    #[test]
    fn find_all_shows_every_match_in_excluded_lines() {
        let mut session = loaded("red\nblue\nred\ngreen\nred\n");
        execute(&mut session, ":2 XX").unwrap();
        execute(&mut session, ":5 XX").unwrap();
        let message = execute(&mut session, "F red ALL").unwrap();
        assert!(message.contains("3 occurrence"), "{message}");
        let doc = &session.current().doc;
        let shown: Vec<_> = (1..=doc.last_line())
            .filter_map(|index| {
                let line = doc.line(index).unwrap();
                if line.is_top() || line.is_bottom() || line.is_xclude() {
                    return None;
                }
                Some((line.text().to_string(), line.is_invisible()))
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                ("red".to_string(), false),
                ("blue".to_string(), true),
                ("red".to_string(), false),
                ("green".to_string(), true),
                ("red".to_string(), false),
            ]
        );
        let hits: Vec<_> = (1..=doc.last_line())
            .filter(|index| !doc.find_spans(*index).is_empty())
            .map(|index| doc.line_text(index).unwrap())
            .collect();
        assert_eq!(hits, vec!["red", "red", "red"]);
        assert_eq!(doc.line_text(doc.cursor().0).unwrap(), "red");
    }

    #[test]
    fn find_pops_an_excluded_line() {
        let mut session = loaded("red\nblue\ngreen\n");
        execute(&mut session, ":2 X").unwrap();
        execute(&mut session, "FIND blue FIRST").unwrap();
        let (line, _) = session.current().doc.cursor();
        assert_eq!(session.current().doc.line_text(line).unwrap(), "blue");
        assert!(!session.current().doc.line(line).unwrap().is_invisible());
    }

    #[test]
    fn copy_after_and_before_and_repeat() {
        let mut session = loaded("a\nb\nc\nd\n");
        execute(&mut session, ":1 C; :2 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\na\nc\nd\n");

        execute(&mut session, "UNDO").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\nc\nd\n");

        execute(&mut session, ":1 C3; :4 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\nc\nd\na\nb\nc\n");

        let mut session = loaded("a\nb\n");
        execute(&mut session, ":1 C; :2 A2").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\na\na\n");

        let mut session = loaded("a\nb\nc\n");
        execute(&mut session, ":3 C; :2 B").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nc\nb\nc\n");
    }

    #[test]
    fn block_copy_and_move_keep_one_label() {
        let mut session = loaded("a\nb\nc\nd\n");
        execute(&mut session, ":1 CC; :3 CC; :4 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\nc\nd\na\nb\nc\n");

        let mut session = loaded("a\nb\nc\n");
        session
            .current_mut()
            .doc
            .set_label(2, Some(".HERE"))
            .unwrap();
        execute(&mut session, ":1 M; :3 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "b\nc\na\n");
        assert_eq!(
            session.current().doc.line(4).unwrap().label.as_deref(),
            Some(".HERE")
        );
        assert!(session.current().doc.line(2).unwrap().label.is_none());

        let mut session = loaded("a\nb\n");
        session
            .current_mut()
            .doc
            .set_label(2, Some(".HERE"))
            .unwrap();
        execute(&mut session, ":1 C; :2 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\na\n");
        assert_eq!(
            session.current().doc.line(2).unwrap().label.as_deref(),
            Some(".HERE")
        );
        assert!(session.current().doc.line(4).unwrap().label.is_none());
    }

    #[test]
    fn copy_stays_pending_until_a_destination() {
        let mut session = loaded("a\nb\nc\n");
        let msg = execute(&mut session, ":2 C").unwrap();
        assert!(msg.contains("Pending destination"), "{msg}");
        assert_eq!(
            session.current().doc.pending().prefix_text(3).as_deref(),
            Some("C")
        );
        execute(&mut session, ":1 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\nb\nc\n");

        let mut session = loaded("a\nb\nc\n");
        execute(&mut session, ":1 CC").unwrap();
        let err = execute(&mut session, ":3 A").unwrap_err();
        assert!(err.message.contains("CC block"), "{}", err.message);
        let mut session = loaded("a\nb\nc\n");
        execute(&mut session, ":1 CC").unwrap();
        execute(&mut session, ":2 CC").unwrap();
        execute(&mut session, ":3 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\nc\na\nb\n");
    }

    #[test]
    fn copy_rejects_overlap_and_two_repeats() {
        let mut session = loaded("a\nb\nc\nd\n");
        let err = execute(&mut session, ":1 CC; :3 CC; :2 A").unwrap_err();
        assert!(err.message.contains("Overlapping"), "{}", err.message);

        let err = execute(&mut session, ":1 CC2; :2 CC; :4 A3").unwrap_err();
        assert!(err.message.contains("repeat"), "{}", err.message);
        assert_eq!(session.current().doc.to_text(), "a\nb\nc\nd\n");
    }

    #[test]
    fn copy_after_an_exclude_lands_past_the_hidden_lines() {
        let mut session = loaded("a\nb\nc\n");
        execute(&mut session, ":2 X").unwrap();
        execute(&mut session, ":1 C; :!3 A").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\na\nc\n");
    }

    #[test]
    fn retyping_a_pending_copy_cancels_it() {
        let mut session = loaded("a\nb\n");
        execute(&mut session, ":1 C").unwrap();
        let msg = execute(&mut session, ":1 C").unwrap();
        assert!(msg.contains("cancelled"), "{msg}");
        assert!(!session.current().doc.pending().is_relocate());
        assert_eq!(session.current().doc.to_text(), "a\nb\n");
    }

    #[test]
    fn repeat_upper_and_shift_commands() {
        let mut session = loaded("ab\ncd\n");
        execute(&mut session, ":1 R2").unwrap();
        assert_eq!(session.current().doc.to_text(), "ab\nab\nab\ncd\n");

        let mut session = loaded("ab\ncd\nef\n");
        execute(&mut session, ":1 RR").unwrap();
        execute(&mut session, ":2 RR").unwrap();
        assert_eq!(session.current().doc.to_text(), "ab\ncd\nab\ncd\nef\n");

        let mut session = loaded("Ab\nCd\n");
        execute(&mut session, ":1 UC2").unwrap();
        assert_eq!(session.current().doc.to_text(), "AB\nCD\n");
        execute(&mut session, ":1 LC").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "ab");

        let mut session = loaded("abcd\n");
        execute(&mut session, ":1 )").unwrap();
        assert_eq!(session.current().doc.to_text(), "  abcd\n");
        execute(&mut session, ":1 (").unwrap();
        assert_eq!(session.current().doc.to_text(), "abcd\n");

        let mut session = loaded("  ab\n");
        execute(&mut session, ":1 >2").unwrap();
        assert_eq!(session.current().doc.to_text(), "    ab\n");
        execute(&mut session, ":1 <3").unwrap();
        assert_eq!(session.current().doc.to_text(), " ab\n");

        let mut session = loaded("ab\ncd\n");
        execute(&mut session, ":1 ((2").unwrap();
        let err = execute(&mut session, ":2 ((3").unwrap_err();
        assert!(err.message.contains("Different"), "{}", err.message);
        assert_eq!(session.current().doc.to_text(), "ab\ncd\n");
    }

    #[test]
    fn delete_exclude_and_sort() {
        let mut session = loaded("b\na\nb\n");
        execute(&mut session, "DELETE b ALL").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\n");

        let mut session = loaded("a\nb\na\n");
        execute(&mut session, "EXCLUDE a ALL").unwrap();
        let doc = &session.current().doc;
        let hidden = (1..doc.last_line()).any(|n| {
            let line = doc.line(n).unwrap();
            line.text() == "a" && line.is_invisible()
        });
        assert!(hidden);

        let mut session = loaded("a\nb\nALL\n");
        let msg = execute(&mut session, "X ALL").unwrap();
        assert_eq!(msg, "Excluded 3 lines");
        assert_eq!(session.current().doc.to_text(), "a\nb\nALL\n");
        let doc = &session.current().doc;
        assert!((1..doc.last_line()).all(|n| {
            let line = doc.line(n).unwrap();
            !line.is_data() || line.is_invisible()
        }));
        assert_eq!(
            (1..doc.last_line())
                .filter(|&n| doc.line(n).unwrap().is_xclude())
                .count(),
            1
        );
        execute(&mut session, "F b").unwrap();
        let doc = &session.current().doc;
        assert!(doc.line(doc.cursor().0).unwrap().text() == "b");
        assert!((1..doc.last_line()).any(|n| {
            let line = doc.line(n).unwrap();
            line.text() == "a" && line.is_invisible()
        }));

        let mut session = loaded("c\na\nb\n");
        execute(&mut session, "SORT").unwrap();
        assert_eq!(session.current().doc.to_text(), "a\nb\nc\n");
        execute(&mut session, "SORT D").unwrap();
        assert_eq!(session.current().doc.to_text(), "c\nb\na\n");
    }

    #[test]
    fn n_inserts_like_i() {
        let mut session = loaded("ab\n");
        execute(&mut session, ":1 N").unwrap();
        assert_eq!(session.current().doc.to_text(), "ab\n\n");
    }

    #[test]
    fn top_and_bottom_move_the_cursor() {
        let mut session = loaded("a\nb\nc\n");
        session.current_mut().doc.set_cursor(3, 2).unwrap();
        let msg = execute(&mut session, "BOTTOM").unwrap();
        assert!(msg.contains("Bottom"), "{msg}");
        assert_eq!(
            session.current().doc.cursor().0,
            session.current().doc.last_line()
        );
        let msg = execute(&mut session, "TOP").unwrap();
        assert!(msg.contains("Top"), "{msg}");
        assert_eq!(session.current().doc.cursor().0, 1);
    }

    #[test]
    fn bookmark_is_set_from_the_number_column_and_found_by_name() {
        let mut session = loaded("a\nb\nc\n");
        let outcome =
            apply_prefixes(&mut session.current_mut().doc, &[(3, ".HERE".to_string())]).unwrap();
        assert!(outcome.message.contains(".HERE"), "{}", outcome.message);
        assert_eq!(
            session.current().doc.line(3).unwrap().label.as_deref(),
            Some(".HERE")
        );
        assert_eq!(
            session
                .current()
                .doc
                .line(3)
                .unwrap()
                .display_prefix()
                .trim(),
            ".HERE"
        );
        execute(&mut session, "BM").unwrap();
        execute(&mut session, ".HERE").unwrap();
        assert_eq!(session.current().doc.cursor().0, 3);
        execute(&mut session, "BM LIST").unwrap();
        let outcome =
            apply_prefixes(&mut session.current_mut().doc, &[(3, ".HERE".to_string())]).unwrap();
        assert!(outcome.message.contains("cleared"), "{}", outcome.message);
        assert!(session.current().doc.line(3).unwrap().label.is_none());
    }

    #[test]
    fn cancel_and_exit_ask_the_window_to_close() {
        let mut session = loaded("keep\n");
        session.current_mut().doc.insert_char(2, 1, 'X').unwrap();
        assert!(session.current().doc.is_modified());
        let msg = execute(&mut session, "CANCEL").unwrap();
        assert_eq!(msg, "Cancelled");
        assert_eq!(
            session.take_pending_close(),
            Some(crate::session::PendingClose::Discard)
        );
        assert!(session.current().doc.is_modified());

        let mut session = loaded("keep\n");
        let msg = execute(&mut session, "EXIT").unwrap();
        assert_eq!(msg, "Exiting");
        assert_eq!(
            session.take_pending_close(),
            Some(crate::session::PendingClose::Save)
        );
    }

    #[test]
    fn remaining_commands_join_overlay_search_and_clip() {
        let mut session = loaded("ab\n");
        let msg = execute(&mut session, "COLS").unwrap();
        assert!(msg.contains("ON"), "{msg}");
        assert!(session.current().doc.cols_mode());
        execute(&mut session, "COLS OFF").unwrap();
        assert!(!session.current().doc.cols_mode());

        let mut session = loaded("Alpha beta\nbeta tail\nplain\n");
        execute(&mut session, "FIND beta").unwrap();
        execute(&mut session, "RFIND").unwrap();
        assert_eq!(session.current().doc.cursor().0, 3);
        execute(&mut session, "RFIND PREV").unwrap();
        assert_eq!(session.current().doc.cursor(), (2, 7));
        execute(&mut session, "RFIND").unwrap();
        assert_eq!(session.current().doc.cursor().0, 3);
        let msg = execute(&mut session, "RFIND").unwrap();
        assert_eq!(msg, "Bottom of data reached");
        assert_eq!(session.current().doc.cursor(), (3, 1));
        let msg = execute(&mut session, "RFIND").unwrap();
        assert!(msg.contains("Found 'beta'"), "{msg}");
        assert_eq!(session.current().doc.cursor(), (2, 7));

        execute(&mut session, "NFIND beta").unwrap();
        let text = session
            .current()
            .doc
            .line_text(session.current().doc.cursor().0)
            .unwrap();
        assert!(!text.to_ascii_lowercase().contains("beta"), "{text}");

        let mut session = loaded("Alpha beta\nkeep me\n");
        execute(&mut session, ":1 J").unwrap();
        assert_eq!(
            session.current().doc.line_text(2).unwrap(),
            "Alpha beta keep me"
        );

        let mut session = loaded("AbC\n  spaced  \n");
        execute(&mut session, "LC").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "abc");
        execute(&mut session, "SC").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "Abc");
        execute(&mut session, ":2 TR").unwrap();
        assert_eq!(session.current().doc.line_text(3).unwrap(), "  Spaced");

        let mut session = loaded("a..b\n....\n");
        execute(&mut session, ":1 C; :2 W").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "....");
        assert_eq!(session.current().doc.line_text(3).unwrap(), "a..b");
        execute(&mut session, ":2 C; :1 O").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "a..b");

        let before = session.current().doc.to_text();
        let msg = execute(&mut session, "CUT").unwrap();
        assert!(msg.contains("Copied"), "{msg}");
        assert_eq!(session.current().doc.to_text(), before);
        execute(&mut session, "PASTE").unwrap();
        assert!(session.current().doc.to_text().contains("a..b"));

        execute(&mut session, ":1 NOTE").unwrap();
        assert!(session
            .current()
            .doc
            .line(3)
            .unwrap()
            .display_prefix()
            .contains("NOTE"));

        let err = execute(&mut session, "HEX").unwrap_err();
        assert_eq!(err.message, "Not supported");
        assert!(!session.current().doc.hex_mode());
        let err = execute(&mut session, "HEX ON").unwrap_err();
        assert_eq!(err.message, "Not supported");
        assert!(!session.current().doc.hex_mode());
        let text = session.current().doc.to_text();
        assert_eq!(execute(&mut session, "SWAPBAR").unwrap(), "SWAPBAR ON");
        assert!(session.swapbar());
        assert_eq!(execute(&mut session, "SWAPBAR").unwrap(), "SWAPBAR OFF");
        assert!(!session.swapbar());
        assert_eq!(execute(&mut session, "SWAPBAR ON").unwrap(), "SWAPBAR ON");
        assert_eq!(
            execute(&mut session, "SWAPBAR /").unwrap_err().message,
            "Not supported"
        );
        assert!(session.swapbar());
        assert_eq!(session.current().doc.to_text(), text);
        execute(&mut session, "UP 1").unwrap();
    }

    #[test]
    fn swap_moves_between_screens_and_opens_the_list() {
        let mut session = loaded("a\n");
        assert_eq!(execute(&mut session, "SWAP").unwrap(), "No other screen");
        assert_eq!(
            execute(&mut session, "SWAP NEXT").unwrap(),
            "No other screen"
        );
        assert_eq!(
            execute(&mut session, "SWAP 2").unwrap_err().message,
            "Screen 2 is not active"
        );

        session.current_mut().title = "PAYROLLS".to_string();
        session.add_screen();
        assert_eq!(session.current_index(), 1);
        assert_eq!(execute(&mut session, "SWAP").unwrap(), "Screen 1 PAYROLLS");
        assert_eq!(session.current_index(), 0);
        assert_eq!(
            execute(&mut session, "SWAP NEXT").unwrap(),
            "Screen 2 Untitled"
        );
        assert_eq!(
            execute(&mut session, "SWAP PREV").unwrap(),
            "Screen 1 PAYROLLS"
        );
        assert_eq!(
            execute(&mut session, "SWAP VERYLONGNAME")
                .unwrap_err()
                .message,
            "Screen 'VERYLONGNAME' is not active"
        );
        session.current_mut().title = "VERYLONGNAME".to_string();
        assert_eq!(
            execute(&mut session, "SWAP VERYLONG").unwrap(),
            "Screen 1 VERYLONGNAME"
        );
        assert_eq!(execute(&mut session, "SWAP LIST").unwrap(), "SWAP LIST");
        assert!(session.swap_list_open());
        assert_eq!(session.current().doc.to_text().contains("a"), true);
        assert_eq!(
            execute(&mut session, "START").unwrap(),
            "Screen 3 Untitled2"
        );
        assert_eq!(session.current_index(), 2);
        assert_eq!(session.current().doc.last_real(), 0);
        assert!(session.take_pending_reveal());
        assert_eq!(
            execute(&mut session, "START extra").unwrap_err().message,
            "START takes no operands"
        );
    }

    #[test]
    fn find_honors_word_prefix_and_column() {
        let mut session = loaded("the cat catch\n");
        let message = execute(&mut session, "F cat WORD").unwrap();
        assert!(message.contains("column 5"), "{message}");
        execute(&mut session, "F cat PREFIX").unwrap();
        assert_eq!(session.current().doc.cursor(), (2, 9));
        assert!(execute(&mut session, "F cat WORD 1").is_err());
        let mut session = loaded("the cat catch\n");
        let changed = execute(&mut session, "C cat CAT WORD").unwrap();
        assert!(changed.contains("Changed"));
        assert!(session.current().doc.to_text().contains("CAT"));
        assert!(session.current().doc.to_text().contains("catch"));
    }

    #[test]
    fn change_parameters_may_appear_in_any_order() {
        let mut session = loaded("cat catch\ncat\n");
        execute(&mut session, "C ALL WORD cat CAT").unwrap();
        assert_eq!(session.current().doc.to_text(), "CAT catch\nCAT\n");

        let mut session = loaded("cat catch\ncat\n");
        execute(&mut session, "C cat ALL CAT WORD").unwrap();
        assert_eq!(session.current().doc.to_text(), "CAT catch\nCAT\n");

        let mut session = loaded("cat catch\ncat\n");
        execute(&mut session, "C PREFIX cat CAT ALL").unwrap();
        assert_eq!(session.current().doc.to_text(), "CAT CATch\nCAT\n");

        let mut named = loaded("ALL\nall\n");
        execute(&mut named, "C ALL XXX").unwrap();
        assert_eq!(named.current().doc.to_text(), "XXX\nall\n");

        let mut quoted = loaded("ALL here\nALL there\n");
        execute(&mut quoted, "C 'ALL' ALL ZZ").unwrap();
        assert_eq!(quoted.current().doc.to_text(), "ZZ here\nZZ there\n");

        let mut ranged = loaded("nope cat\ncat inside\ncat inside\ncat after\n");
        execute(&mut ranged, ":2 .start").unwrap();
        execute(&mut ranged, ":3 .End").unwrap();
        let changed = execute(&mut ranged, "C .StArT ALL cat .eNd CAT NX").unwrap();
        assert!(changed.contains("2 occurrence"), "{changed}");
        assert_eq!(
            ranged.current().doc.to_text(),
            "nope cat\nCAT inside\nCAT inside\ncat after\n"
        );

        let mut columns = loaded("cat cat\n");
        execute(&mut columns, "C 5 FIRST cat CAT").unwrap();
        assert_eq!(columns.current().doc.to_text(), "cat CAT\n");

        let mut excluded = loaded("cat\ndog\n");
        execute(&mut excluded, ":1 X").unwrap();
        execute(&mut excluded, "C X LAST cat CAT").unwrap();
        assert_eq!(excluded.current().doc.to_text(), "CAT\ndog\n");

        let mut suffix = loaded("bobcat cat\n");
        execute(&mut suffix, "C SUFFIX ALL cat DOG 1 6").unwrap();
        assert_eq!(suffix.current().doc.to_text(), "bobDOG cat\n");

        let mut clash = loaded("cat\n");
        assert_eq!(
            execute(&mut clash, "C cat CAT ALL FIRST")
                .unwrap_err()
                .message,
            "Inconsistent parameters"
        );
        assert_eq!(
            execute(&mut clash, "C cat CAT WORD PREFIX")
                .unwrap_err()
                .message,
            "Inconsistent parameters"
        );
        assert_eq!(
            execute(&mut clash, "C cat CAT X NX").unwrap_err().message,
            "Inconsistent parameters"
        );
    }

    #[test]
    fn find_and_change_labels_ignore_case() {
        let mut session = loaded("nope cat\ncat inside\ncat inside\ncat after\n");
        execute(&mut session, ":2 .start").unwrap();
        execute(&mut session, ":3 .End").unwrap();
        let message = execute(&mut session, "F cat .StArT .eNd ALL").unwrap();
        assert!(message.contains("2 occurrence"), "{message}");
        let changed = execute(&mut session, "C cat CAT .START .end ALL").unwrap();
        assert!(changed.contains("2 occurrence"), "{changed}");
        assert_eq!(
            session.current().doc.to_text(),
            "nope cat\nCAT inside\nCAT inside\ncat after\n"
        );

        let mut session = loaded("nope cat\ncat inside\ncat inside\ncat after\n");
        execute(&mut session, ":2 .A").unwrap();
        execute(&mut session, ":3 .B").unwrap();
        let message = execute(&mut session, "F ALL .a .b cat").unwrap();
        assert!(message.contains("2 occurrence"), "{message}");
        let swapped = execute(&mut session, "F .b .a ALL cat").unwrap();
        assert!(swapped.contains("2 occurrence"), "{swapped}");
        let missing = execute(&mut session, "F ALL .a .b XXXX").unwrap_err();
        assert!(missing.message.contains("not found"), "{}", missing.message);
    }

    #[test]
    fn cut_copies_every_line_or_the_c_range_and_rejects_a_destination() {
        let mut session = loaded("one\ntwo\nthree\n");
        execute(&mut session, "CUT").unwrap();
        assert_eq!(session.clip(), ["one", "two", "three"]);

        execute(&mut session, ":2 C").unwrap();
        execute(&mut session, "CUT").unwrap();
        assert_eq!(session.clip(), ["two"]);
        assert_eq!(
            session.current().doc.pending(),
            crate::document::PendingBlock::None
        );

        execute(&mut session, ":3 A").unwrap();
        assert_eq!(
            execute(&mut session, "CUT").unwrap_err().message,
            "command conflict"
        );

        let mut session = loaded("a\nb\n");
        let msg = execute(&mut session, "RENUM").unwrap();
        assert!(msg.contains("STD"), "{msg}");
        assert_eq!(session.current().doc.line(2).unwrap().seq, 100);
        assert_eq!(session.current().doc.line(3).unwrap().seq, 200);
        assert_eq!(session.current().doc.line_text(2).unwrap(), "a");
        assert!(!session.current().doc.is_modified());
        session
            .current_mut()
            .doc
            .insert_lines_after(2, 1, false)
            .unwrap();
        assert_eq!(session.current().doc.line(2).unwrap().seq, 100);
        assert_eq!(session.current().doc.line(3).unwrap().seq, 200);
        assert_eq!(session.current().doc.line(4).unwrap().seq, 300);

        let mut session = loaded("abcdef\n");
        execute(&mut session, "RENUM COBOL").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "000100");
        assert_eq!(session.current().doc.line(2).unwrap().seq, 100);
        assert!(session.current().doc.is_modified());
        session.current_mut().doc.undo().unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "abcdef");
        assert_eq!(session.current().doc.line(2).unwrap().seq, 1);
        assert!(!session.current().doc.is_modified());

        let mut session = loaded("abcdefg\n");
        execute(&mut session, "RENUM STD COBOL DISPLAY").unwrap();
        assert_eq!(session.current().doc.line_text(2).unwrap(), "000100g");
        assert!(session.current().doc.number_display());
        execute(&mut session, "RENUM OFF").unwrap();
        assert_eq!(session.current().doc.line(2).unwrap().seq, 1);
        assert!(session.current().doc.number_kind().is_none());
        assert_eq!(session.current().doc.line_text(2).unwrap(), "000100g");

        let err = execute(&mut session, "RENUM BOGUS").unwrap_err();
        assert!(err.message.contains("Unknown keyword"), "{}", err.message);
        assert_eq!(crate::document::Document::ispf_increment(3, false), 100);
        assert_eq!(
            crate::document::Document::ispf_increment(10_000, false),
            100
        );
        assert_eq!(crate::document::Document::ispf_increment(10_001, false), 99);
        assert_eq!(crate::document::Document::ispf_increment(10_000, true), 99);
        assert_eq!(crate::document::Document::ispf_increment(5_000, true), 100);
    }

    #[test]
    fn reset_clears_each_ispf_kind_and_keeps_labels() {
        let mut session = loaded("one\ntwo\nthree\n");
        execute(&mut session, "CHANGE one ONE").unwrap();
        assert!(session
            .current()
            .doc
            .line(2)
            .unwrap()
            .display_prefix()
            .contains("CHG"));
        execute(&mut session, "RESET CHANGE").unwrap();
        assert!(!session
            .current()
            .doc
            .line(2)
            .unwrap()
            .flags
            .contains(crate::line::LineFlags::EQ_CHANGE));
        assert_eq!(
            session.current().doc.line(2).unwrap().display_prefix(),
            "00000001"
        );

        execute(&mut session, "LABEL 1 .A").unwrap();
        execute(&mut session, "LABEL 2 .B").unwrap();
        execute(&mut session, "RESET").unwrap();
        assert_eq!(
            session.current().doc.line(2).unwrap().label.as_deref(),
            Some(".A")
        );
        assert_eq!(
            session.current().doc.line(3).unwrap().label.as_deref(),
            Some(".B")
        );
        execute(&mut session, "RESET LABEL .A .A").unwrap();
        assert!(session.current().doc.line(2).unwrap().label.is_none());
        assert_eq!(
            session.current().doc.line(3).unwrap().label.as_deref(),
            Some(".B")
        );
        execute(&mut session, "RESET ALL").unwrap();
        assert!(session.current().doc.line(3).unwrap().label.is_none());

        execute(&mut session, ":1 C").unwrap();
        execute(&mut session, "FIND two").unwrap();
        assert!(!session.current().doc.find_spans(3).is_empty());
        execute(&mut session, "RES FIND").unwrap();
        assert!(session.current().doc.find_spans(3).is_empty());
        assert_ne!(session.current().doc.pending(), PendingBlock::None);
        execute(&mut session, "RESET COMMAND").unwrap();
        assert_eq!(session.current().doc.pending(), PendingBlock::None);

        execute(&mut session, ":3 X").unwrap();
        execute(&mut session, "RES FIND").unwrap();
        assert!(session
            .current()
            .doc
            .visible_rows()
            .iter()
            .any(|row| row.exclude_count.is_some()));
        execute(&mut session, "RESET EXCLUDED").unwrap();
        assert!(session
            .current()
            .doc
            .visible_rows()
            .iter()
            .all(|row| row.exclude_count.is_none()));

        execute(&mut session, "COLS").unwrap();
        execute(&mut session, ":1 NOTE").unwrap();
        execute(&mut session, "RES SPECIAL").unwrap();
        assert!(!session.current().doc.cols_mode());
        assert!(session
            .current()
            .doc
            .lines
            .iter()
            .all(|line| !line.is_note()));

        execute(&mut session, "RESET").unwrap();
        execute(&mut session, "RESET ERROR").unwrap();
        let err = execute(&mut session, "RES BOGUS").unwrap_err();
        assert!(err.message.contains("Unknown keyword"), "{}", err.message);
        let err = execute(&mut session, "RESET .A").unwrap_err();
        assert!(err.message.contains("two labels"), "{}", err.message);
    }
}
