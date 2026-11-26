// SPDX-License-Identifier: GPL-3.0-or-later
//! COBOL divisions, sections, and paragraphs for the outline panel.
//!
//! Fixed-format lines use columns 1–6 as the sequence area and column 7 as
//! the indicator. A header must start in area A. Area B statements such as
//! `EXIT.` are not headings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::document::Document;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CobolKind {
    Division,
    Section,
    Paragraph,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CobolEntry {
    pub line: usize,
    pub name: String,
    pub kind: CobolKind,
}

impl CobolEntry {
    pub fn label(&self) -> String {
        let name = self.name.to_ascii_uppercase();
        match self.kind {
            CobolKind::Division => format!("{name} DIVISION"),
            CobolKind::Section => format!("  {name} SECTION"),
            CobolKind::Paragraph => format!("    {name}"),
        }
    }
}

pub fn cobol_path(path: Option<&Path>) -> bool {
    let Some(ext) = path
        .and_then(|path| path.extension())
        .and_then(|ext| ext.to_str())
    else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "cbl" | "cob" | "cobol" | "cpy" | "sqb" | "pco" | "sco"
    )
}

pub fn cobol_outline(doc: &Document) -> Vec<CobolEntry> {
    let mut entries = Vec::new();
    for line in 1..doc.last_line() {
        let Ok(row) = doc.line(line) else {
            continue;
        };
        if !row.is_data() {
            continue;
        }
        if let Some((name, kind)) = heading(row.text()) {
            entries.push(CobolEntry { line, name, kind });
        }
    }
    entries
}

/// Where a name is defined. `path` is set when the definition is in a copybook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefSite {
    pub line: usize,
    pub col: usize,
    pub path: Option<PathBuf>,
}

/// First paragraph or section of each name. Divisions are not call targets.
pub fn paragraph_index(entries: &[CobolEntry]) -> HashMap<String, DefSite> {
    let mut targets = HashMap::new();
    for entry in entries {
        if matches!(entry.kind, CobolKind::Paragraph | CobolKind::Section) {
            targets
                .entry(entry.name.to_ascii_uppercase())
                .or_insert(DefSite {
                    line: entry.line,
                    col: 1,
                    path: None,
                });
        }
    }
    targets
}

/// A paragraph or section name used by `PERFORM`, `GO TO`, `GOTO`, `THRU`, or `THROUGH`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParagraphCall {
    /// 1-based column of the name in the full line.
    pub col: usize,
    pub len: usize,
    /// Line and column of the definition.
    pub line: usize,
    pub target_col: usize,
    /// File that holds the definition. `None` means the current tab.
    pub path: Option<PathBuf>,
}

pub fn paragraph_calls(line: &str, targets: &HashMap<String, DefSite>) -> Vec<ParagraphCall> {
    let chars: Vec<char> = line.chars().collect();
    let Some((start, end)) = call_span(&chars) else {
        return Vec::new();
    };
    let mut calls = Vec::new();
    let mut i = start;
    let mut expect_name = false;
    let mut after_go = false;
    while i < end {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            i = skip_string(&chars, i, end);
            expect_name = false;
            after_go = false;
            continue;
        }
        if ch == '*' && i + 1 < end && chars[i + 1] == '>' {
            break;
        }
        let Some((word, next)) = take_word(&chars, i, end) else {
            expect_name = false;
            after_go = false;
            i += 1;
            continue;
        };
        let upper = word.to_ascii_uppercase();
        if after_go {
            after_go = false;
            expect_name = upper == "TO";
            i = next;
            continue;
        }
        if matches!(upper.as_str(), "PERFORM" | "THRU" | "THROUGH" | "GOTO") {
            expect_name = true;
            i = next;
            continue;
        }
        if upper == "GO" {
            after_go = true;
            expect_name = false;
            i = next;
            continue;
        }
        if expect_name && !crate::highlight::cobol_keyword(&upper) {
            if let Some(target) = targets.get(&upper) {
                calls.push(ParagraphCall {
                    col: i + 1,
                    len: word.chars().count(),
                    line: target.line,
                    target_col: target.col,
                    path: target.path.clone(),
                });
            }
        }
        expect_name = false;
        i = next;
    }
    calls
}

/// Where a data name is defined. `col` is the 1-based column of that name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableDef {
    pub line: usize,
    pub col: usize,
    pub path: Option<PathBuf>,
}

/// Names from this file plus any `{name}.cbl` copybooks found beside it.
#[derive(Clone, Debug, Default)]
pub struct NameIndex {
    pub paragraphs: HashMap<String, DefSite>,
    pub variables: HashMap<String, VariableDef>,
    pub copies: HashMap<String, PathBuf>,
}

/// First definition of each data name. `FILLER` is not a target.
pub fn variable_index(doc: &Document) -> HashMap<String, VariableDef> {
    let mut defs = HashMap::new();
    let mut pending_name = false;
    for line_no in 1..doc.last_line() {
        let Ok(row) = doc.line(line_no) else {
            continue;
        };
        if !row.is_data() {
            continue;
        }
        let chars: Vec<char> = row.text().chars().collect();
        let Some((start, end)) = call_span(&chars) else {
            pending_name = false;
            continue;
        };
        // A level number starts a data entry. Picture digits such as 9(4) do not.
        let mut at_entry = true;
        let mut i = start;
        while i < end {
            let ch = chars[i];
            if ch.is_whitespace() {
                i += 1;
                continue;
            }
            if ch == '.' {
                at_entry = true;
                pending_name = false;
                i += 1;
                continue;
            }
            if ch == '"' || ch == '\'' {
                i = skip_string(&chars, i, end);
                continue;
            }
            if ch == '*' && i + 1 < end && chars[i + 1] == '>' {
                break;
            }
            if let Some((word, next)) = take_token(&chars, i, end) {
                if pending_name {
                    pending_name = false;
                    if is_data_name(&word) {
                        defs.entry(word.to_ascii_uppercase())
                            .or_insert(VariableDef {
                                line: line_no,
                                col: i + 1,
                                path: None,
                            });
                    }
                } else if at_entry && is_level(&word) {
                    pending_name = true;
                }
                at_entry = false;
                i = next;
                continue;
            }
            at_entry = false;
            i += 1;
        }
    }
    defs
}

/// Local names plus variables and paragraphs from `{copybook}.cbl` in `folder`.
pub fn name_index(doc: &Document, folder: Option<&Path>) -> NameIndex {
    let mut paragraphs = paragraph_index(&cobol_outline(doc));
    let mut variables = variable_index(doc);
    let mut copies = HashMap::new();
    let Some(folder) = folder else {
        return NameIndex {
            paragraphs,
            variables,
            copies,
        };
    };
    for name in copy_names_in(doc) {
        let upper = name.to_ascii_uppercase();
        if copies.contains_key(&upper) {
            continue;
        }
        let Some(path) = resolve_copybook(folder, &name) else {
            continue;
        };
        copies.insert(upper.clone(), path.clone());
        let Ok(copy_doc) = Document::load_path(&path) else {
            continue;
        };
        for (key, mut site) in paragraph_index(&cobol_outline(&copy_doc)) {
            site.path = Some(path.clone());
            paragraphs.entry(key).or_insert(site);
        }
        for (key, mut def) in variable_index(&copy_doc) {
            def.path = Some(path.clone());
            variables.entry(key).or_insert(def);
        }
    }
    NameIndex {
        paragraphs,
        variables,
        copies,
    }
}

/// The variable, paragraph, or copybook named by the code word at `col`.
///
/// A call or a use is chosen first, the same way Ctrl-click chooses. The
/// defining word itself is included too. Reserved words are not targets.
pub fn definition_at(
    line: &str,
    line_no: usize,
    col: usize,
    variables: &HashMap<String, VariableDef>,
    paragraphs: &HashMap<String, DefSite>,
    copies: &HashMap<String, PathBuf>,
) -> Option<ParagraphCall> {
    if let Some(link) = copy_name_links(line, copies)
        .into_iter()
        .find(|link| col >= link.col && col < link.col + link.len)
    {
        return Some(link);
    }
    if let Some(link) = paragraph_calls(line, paragraphs)
        .into_iter()
        .find(|link| col >= link.col && col < link.col + link.len)
    {
        return Some(link);
    }
    if let Some(link) = variable_uses(line, line_no, variables)
        .into_iter()
        .find(|link| col >= link.col && col < link.col + link.len)
    {
        return Some(link);
    }
    let mut found = None;
    each_code_token(line, |col0, word| {
        let start = col0 + 1;
        let len = word.chars().count();
        if col >= start && col < start + len {
            found = Some((start, len, word.to_string()));
        }
    });
    let (start, len, word) = found?;
    if crate::highlight::cobol_keyword(&word) {
        return None;
    }
    let upper = word.to_ascii_uppercase();
    if let Some(path) = copies.get(&upper) {
        return Some(ParagraphCall {
            col: start,
            len,
            line: 2,
            target_col: 1,
            path: Some(path.clone()),
        });
    }
    if let Some(target) = paragraphs.get(&upper) {
        return Some(ParagraphCall {
            col: start,
            len,
            line: target.line,
            target_col: target.col,
            path: target.path.clone(),
        });
    }
    let def = variables.get(&upper)?;
    Some(ParagraphCall {
        col: start,
        len,
        line: def.line,
        target_col: def.col,
        path: def.path.clone(),
    })
}

/// Uses of a data name defined in this file or a loaded copybook.
/// The defining occurrence in the current file itself is not a link.
pub fn variable_uses(
    line: &str,
    line_no: usize,
    defs: &HashMap<String, VariableDef>,
) -> Vec<ParagraphCall> {
    let mut uses = Vec::new();
    each_code_token(line, |col0, word| {
        if !is_data_name(word) {
            return;
        }
        let Some(def) = defs.get(&word.to_ascii_uppercase()) else {
            return;
        };
        let col = col0 + 1;
        if def.path.is_none() && def.line == line_no && def.col == col {
            return;
        }
        uses.push(ParagraphCall {
            col,
            len: word.chars().count(),
            line: def.line,
            target_col: def.col,
            path: def.path.clone(),
        });
    });
    uses
}

/// The copybook name on a `COPY` or `COPYBOOK` line, when that file was found.
pub fn copy_name_links(line: &str, copies: &HashMap<String, PathBuf>) -> Vec<ParagraphCall> {
    let mut links = Vec::new();
    for (col, len, name) in copy_names_on_line(line) {
        let Some(path) = copies.get(&name.to_ascii_uppercase()) else {
            continue;
        };
        links.push(ParagraphCall {
            col,
            len,
            line: 2,
            target_col: 1,
            path: Some(path.clone()),
        });
    }
    links
}

fn copy_names_in(doc: &Document) -> Vec<String> {
    let mut names = Vec::new();
    for line in 1..doc.last_line() {
        let Ok(row) = doc.line(line) else {
            continue;
        };
        if !row.is_data() {
            continue;
        }
        for (_, _, name) in copy_names_on_line(row.text()) {
            if !names
                .iter()
                .any(|have: &String| have.eq_ignore_ascii_case(&name))
            {
                names.push(name);
            }
        }
    }
    names
}

fn copy_names_on_line(line: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let Some((start, end)) = call_span(&chars) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut i = start;
    let mut expect_name = false;
    while i < end {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let open = i;
            i += 1;
            while i < end && chars[i] != quote {
                i += 1;
            }
            if expect_name && i < end {
                let text: String = chars[open + 1..i].iter().collect();
                let name = text.trim();
                if !name.is_empty() {
                    names.push((open + 2, name.chars().count(), name.to_string()));
                }
            }
            expect_name = false;
            i = (i + 1).min(end);
            continue;
        }
        if ch == '*' && i + 1 < end && chars[i + 1] == '>' {
            break;
        }
        let Some((word, next)) = take_word(&chars, i, end) else {
            expect_name = false;
            i += 1;
            continue;
        };
        let upper = word.to_ascii_uppercase();
        if matches!(upper.as_str(), "COPY" | "COPYBOOK") {
            expect_name = true;
            i = next;
            continue;
        }
        if expect_name {
            names.push((i + 1, word.chars().count(), word));
            expect_name = false;
        }
        i = next;
    }
    names
}

fn resolve_copybook(folder: &Path, name: &str) -> Option<PathBuf> {
    let exact = folder.join(format!("{name}.cbl"));
    if exact.is_file() {
        return Some(exact);
    }
    let want = format!("{name}.cbl");
    let entries = std::fs::read_dir(folder).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.eq_ignore_ascii_case(&want) && path.is_file() {
            return Some(path);
        }
    }
    None
}

fn is_level(word: &str) -> bool {
    if !word.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    let Ok(level) = word.parse::<u16>() else {
        return false;
    };
    (1..=49).contains(&level) || matches!(level, 66 | 77 | 88)
}

fn is_data_name(word: &str) -> bool {
    !word.eq_ignore_ascii_case("FILLER")
        && !crate::highlight::cobol_keyword(word)
        && word.chars().any(|ch| ch.is_ascii_alphabetic())
        && !word.starts_with('-')
        && !word.ends_with('-')
        && word
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

fn each_code_token(line: &str, mut visit: impl FnMut(usize, &str)) {
    let chars: Vec<char> = line.chars().collect();
    let Some((start, end)) = call_span(&chars) else {
        return;
    };
    let mut i = start;
    while i < end {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            i = skip_string(&chars, i, end);
            continue;
        }
        if ch == '*' && i + 1 < end && chars[i + 1] == '>' {
            break;
        }
        if let Some((word, next)) = take_token(&chars, i, end) {
            visit(i, &word);
            i = next;
            continue;
        }
        i += 1;
    }
}

fn take_token(chars: &[char], start: usize, end: usize) -> Option<(String, usize)> {
    let ch = chars[start];
    if !(ch.is_ascii_alphanumeric() || ch == '-') {
        return None;
    }
    let mut i = start + 1;
    while i < end && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
        i += 1;
    }
    Some((chars[start..i].iter().collect(), i))
}

fn heading(line: &str) -> Option<(String, CobolKind)> {
    let body = code_body(line)?;
    let lead = body.chars().take_while(|ch| ch.is_whitespace()).count();
    if lead >= 4 {
        return None;
    }
    let text = body.trim();
    if text.is_empty() || text.starts_with("*>") {
        return None;
    }
    let (name, rest) = cobol_word(text)?;
    if rest_is(rest, "DIVISION") {
        return Some((name.to_string(), CobolKind::Division));
    }
    if rest_is(rest, "SECTION") {
        return Some((name.to_string(), CobolKind::Section));
    }
    if rest.starts_with('.') {
        return Some((name.to_string(), CobolKind::Paragraph));
    }
    None
}

fn rest_is(rest: &str, keyword: &str) -> bool {
    let rest = rest.trim_start();
    if rest.len() < keyword.len() || !rest[..keyword.len()].eq_ignore_ascii_case(keyword) {
        return false;
    }
    let after = &rest[keyword.len()..];
    let boundary = after.is_empty()
        || after.starts_with('.')
        || after.starts_with(|ch: char| ch.is_whitespace());
    boundary && rest.contains('.')
}

fn cobol_word(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let end = text
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let name = &text[..end];
    if name.starts_with('-') || name.ends_with('-') {
        return None;
    }
    if !name.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }
    Some((name, text[end..].trim_start()))
}

/// Code from area A through column 72. Comments and continuations are skipped.
fn code_body(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    if let Some(body) = fixed_body(&chars) {
        return body;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with("*>") || trimmed.starts_with('*') {
        return None;
    }
    Some(line.to_string())
}

/// Numbered columns 1–6, or a blank sequence area with the text starting in area A.
fn fixed_body(chars: &[char]) -> Option<Option<String>> {
    if chars.len() < 7 {
        return None;
    }
    let numbered = chars[..6].iter().all(|ch| ch.is_ascii_digit());
    let blank_area_a =
        chars.len() >= 8 && chars[..6].iter().all(|ch| *ch == ' ') && chars[7] != ' ';
    if !numbered && !blank_area_a {
        return None;
    }
    let indicator = chars[6];
    if !matches!(indicator, ' ' | '*' | '/' | '-' | 'D' | 'd') {
        return None;
    }
    if indicator == '*' || indicator == '/' || indicator == '-' {
        return Some(None);
    }
    let end = chars.len().min(7 + 65);
    if end <= 7 {
        return Some(None);
    }
    Some(Some(chars[7..end].iter().collect()))
}

fn call_span(chars: &[char]) -> Option<(usize, usize)> {
    if chars.len() >= 7 {
        let numbered = chars[..6].iter().all(|ch| ch.is_ascii_digit());
        let blank_area_a =
            chars.len() >= 8 && chars[..6].iter().all(|ch| *ch == ' ') && chars[7] != ' ';
        if (numbered || blank_area_a) && matches!(chars[6], ' ' | '*' | '/' | '-' | 'D' | 'd') {
            if chars[6] == '*' || chars[6] == '/' || chars[6] == '-' {
                return None;
            }
            return Some((7, chars.len().min(72)));
        }
    }
    let line: String = chars.iter().collect();
    let trimmed = line.trim_start();
    if trimmed.starts_with("*>") || trimmed.starts_with('*') {
        return None;
    }
    Some((0, chars.len()))
}

fn skip_string(chars: &[char], start: usize, end: usize) -> usize {
    let quote = chars[start];
    let mut i = start + 1;
    while i < end {
        if chars[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    end
}

fn take_word(chars: &[char], start: usize, end: usize) -> Option<(String, usize)> {
    let ch = chars[start];
    if !(ch.is_ascii_alphanumeric() || ch == '-') {
        return None;
    }
    let mut i = start + 1;
    while i < end && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
        i += 1;
    }
    let word: String = chars[start..i].iter().collect();
    if word.starts_with('-')
        || word.ends_with('-')
        || !word.chars().any(|ch| ch.is_ascii_alphabetic())
    {
        return None;
    }
    Some((word, i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn fixed_format_lists_divisions_sections_and_paragraphs() {
        let source = "\
000100 IDENTIFICATION DIVISION.\n\
000200 PROGRAM-ID. SAMPLE.\n\
000300 DATA DIVISION.\n\
000400 WORKING-STORAGE SECTION.\n\
000500 01  WS-COUNT PIC 9(4).\n\
000600 PROCEDURE DIVISION.\n\
000700 1000-MAIN SECTION.\n\
000800 1000-START.\n\
000900     EXIT.\n\
001000*    SKIPPED.\n\
001100 2000-READ.\n";
        let doc = Document::from_text(source);
        let entries = cobol_outline(&doc);
        let names: Vec<_> = entries.iter().map(|entry| entry.label()).collect();
        assert_eq!(
            names,
            vec![
                "IDENTIFICATION DIVISION",
                "    PROGRAM-ID",
                "DATA DIVISION",
                "  WORKING-STORAGE SECTION",
                "PROCEDURE DIVISION",
                "  1000-MAIN SECTION",
                "    1000-START",
                "    2000-READ",
            ]
        );
        assert!(entries.iter().all(|entry| entry.line >= 2));
    }

    #[test]
    fn free_format_skips_area_b_and_comments() {
        let source = "identification division.\nprogram-id. hello.\nprocedure division using lk-parm.\nmain-para.\n    display 'hi'.\n        exit.\n*> comment\n2000-end section.\n";
        let doc = Document::from_text(source);
        let names: Vec<_> = cobol_outline(&doc)
            .into_iter()
            .map(|entry| entry.label())
            .collect();
        assert_eq!(
            names,
            vec![
                "IDENTIFICATION DIVISION",
                "    PROGRAM-ID",
                "PROCEDURE DIVISION",
                "    MAIN-PARA",
                "  2000-END SECTION",
            ]
        );
    }

    #[test]
    fn perform_and_go_to_point_at_paragraphs_in_the_file() {
        let source = "procedure division.\n1000-main section.\n1000-start.\n    perform 1000-start.\n    perform 1000-main thru 2000-end.\n    go to 2000-end.\n    display 'perform 1000-start'.\n2000-end.\n";
        let doc = Document::from_text(source);
        let targets = paragraph_index(&cobol_outline(&doc));
        let calls = paragraph_calls("    perform 1000-start.", &targets);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].col, 13);
        assert_eq!(calls[0].len, 10);
        assert_eq!(doc.line(calls[0].line).unwrap().text(), "1000-start.");

        let thru = paragraph_calls("    perform 1000-main thru 2000-end.", &targets);
        assert_eq!(thru.len(), 2);
        assert_eq!(doc.line(thru[0].line).unwrap().text(), "1000-main section.");
        assert_eq!(doc.line(thru[1].line).unwrap().text(), "2000-end.");

        let go = paragraph_calls("    go to 2000-end.", &targets);
        assert_eq!(go.len(), 1);
        assert_eq!(doc.line(go[0].line).unwrap().text(), "2000-end.");

        assert!(paragraph_calls("    display 'perform 1000-start'.", &targets).is_empty());
        assert!(paragraph_calls("1000-start.", &targets).is_empty());
        assert!(paragraph_calls("    perform until done", &targets).is_empty());
    }

    #[test]
    fn variable_uses_jump_to_the_definition_in_the_file() {
        let source = "data division.\nworking-storage section.\n01  ws-name pic x(10).\n05  ws-count pic 9(4).\n01  filler pic x.\n77  ws-flag pic x.\nprocedure division.\n    move ws-name to ws-count.\n    display 'ws-name'.\n    if ws-flag = 'Y'\n        move ws-name to ws-flag.\n";
        let doc = Document::from_text(source);
        let defs = variable_index(&doc);
        assert!(defs.get("FILLER").is_none());
        let name = defs.get("WS-NAME").unwrap();
        assert_eq!(
            doc.line(name.line).unwrap().text(),
            "01  ws-name pic x(10)."
        );
        assert!(variable_uses("01  ws-name pic x(10).", name.line, &defs).is_empty());

        let uses = variable_uses("    move ws-name to ws-count.", 0, &defs);
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[0].col, 10);
        assert_eq!(uses[0].len, 7);
        assert_eq!(uses[0].line, name.line);
        assert_eq!(uses[0].target_col, name.col);
        assert_eq!(
            doc.line(uses[1].line).unwrap().text(),
            "05  ws-count pic 9(4)."
        );
        assert!(variable_uses("    display 'ws-name'.", 0, &defs).is_empty());
    }

    #[test]
    fn ctrl_links_skip_cobol_keywords() {
        let source = "\
data division.\n\
working-storage section.\n\
01  compute pic x.\n\
88  end-if value 'y'.\n\
01  ws-amt pic 9(4).\n\
procedure division.\n\
compute.\n\
    compute ws-amt = 1\n\
    move compute to end-if\n\
    perform compute\n\
    end-if.\n";
        let doc = Document::from_text(source);
        let defs = variable_index(&doc);
        assert!(defs.get("COMPUTE").is_none());
        assert!(defs.get("END-IF").is_none());
        assert!(defs.get("WS-AMT").is_some());
        let uses = variable_uses("    move compute to end-if", 0, &defs);
        assert!(uses.is_empty());
        let amt = variable_uses("    compute ws-amt = 1", 0, &defs);
        assert_eq!(amt.len(), 1);
        assert_eq!(amt[0].len, 6);
        let targets = paragraph_index(&cobol_outline(&doc));
        assert!(targets.get("COMPUTE").is_some());
        assert!(paragraph_calls("    perform compute", &targets).is_empty());
    }

    #[test]
    fn definition_at_finds_a_variable_or_paragraph_and_skips_keywords() {
        let source = "\
data division.\n\
working-storage section.\n\
01  ws-amt pic 9(4).\n\
procedure division.\n\
1000-main.\n\
    move ws-amt to ws-amt\n\
    perform 1000-main\n\
    compute ws-amt = 1\n\
    end-if.\n";
        let doc = Document::from_text(source);
        let defs = variable_index(&doc);
        let targets = paragraph_index(&cobol_outline(&doc));
        let defined = doc
            .line(defs.get("WS-AMT").unwrap().line)
            .unwrap()
            .text()
            .to_string();
        let copies = HashMap::new();
        let on_definition = definition_at(
            &defined,
            0,
            defined.find("ws-amt").unwrap() + 1,
            &defs,
            &targets,
            &copies,
        )
        .unwrap();
        assert_eq!(on_definition.line, defs.get("WS-AMT").unwrap().line);
        assert_eq!(on_definition.target_col, defs.get("WS-AMT").unwrap().col);

        let uses = "    move ws-amt to ws-amt";
        let use_at = definition_at(
            uses,
            0,
            uses.find("ws-amt").unwrap() + 1,
            &defs,
            &targets,
            &copies,
        )
        .unwrap();
        assert_eq!(use_at.line, defs.get("WS-AMT").unwrap().line);

        let heading = "1000-main.";
        let para = definition_at(heading, 0, 1, &defs, &targets, &copies).unwrap();
        assert_eq!(doc.line(para.line).unwrap().text(), "1000-main.");

        let call = "    perform 1000-main";
        let called = definition_at(
            call,
            0,
            call.find("1000-main").unwrap() + 1,
            &defs,
            &targets,
            &copies,
        )
        .unwrap();
        assert_eq!(called.line, para.line);

        assert!(definition_at("    end-if.", 0, 5, &defs, &targets, &copies).is_none());
        assert!(definition_at("    display 'ws-amt'.", 0, 15, &defs, &targets, &copies).is_none());
    }

    #[test]
    fn copybook_names_merge_into_the_index() {
        let dir = std::env::temp_dir().join(format!("ispflite-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join("WSCOMM.cbl");
        std::fs::write(&copy, "01  ws-shared pic x(10).\ncopy-para.\n    exit.\n").unwrap();
        let source = "data division.\nworking-storage section.\n    copy wscomm.\nprocedure division.\n    move ws-shared to ws-shared\n    perform copy-para\n";
        let doc = Document::from_text(source);
        let index = name_index(&doc, Some(&dir));
        assert!(index.copies.contains_key("WSCOMM"));
        let shared = index
            .variables
            .get("WS-SHARED")
            .unwrap()
            .path
            .as_ref()
            .unwrap();
        let para = index
            .paragraphs
            .get("COPY-PARA")
            .unwrap()
            .path
            .as_ref()
            .unwrap();
        assert!(crate::session::same_file(shared, &copy));
        assert!(crate::session::same_file(para, &copy));
        let links = copy_name_links("    copy wscomm.", &index.copies);
        assert_eq!(links.len(), 1);
        assert!(crate::session::same_file(
            links[0].path.as_ref().unwrap(),
            &copy
        ));
        let uses = variable_uses("    move ws-shared to ws-shared", 0, &index.variables);
        assert!(!uses.is_empty());
        assert!(crate::session::same_file(
            uses[0].path.as_ref().unwrap(),
            &copy
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ctrl_links_skip_every_cobol_keyword() {
        let mut source = String::from("data division.\nworking-storage section.\n");
        for word in crate::highlight::cobol_keywords() {
            source.push_str(&format!("01  {word} pic x.\n"));
        }
        source.push_str("01  ws-amt pic 9(4).\n");
        let doc = Document::from_text(&source);
        let mut defs = variable_index(&doc);
        let mut targets = HashMap::new();
        for word in crate::highlight::cobol_keywords() {
            assert!(
                defs.get(*word).is_none(),
                "{word} was indexed as a data name"
            );
            defs.insert(
                (*word).to_string(),
                VariableDef {
                    line: 2,
                    col: 1,
                    path: None,
                },
            );
            targets.insert(
                (*word).to_string(),
                DefSite {
                    line: 2,
                    col: 1,
                    path: None,
                },
            );
            assert!(
                variable_uses(&format!("    {word}"), 0, &defs).is_empty(),
                "{word} was underlined as a data name"
            );
            assert!(
                paragraph_calls(&format!("    perform {word}"), &targets).is_empty(),
                "perform {word} was underlined"
            );
            assert!(
                paragraph_calls(&format!("    go to {word}"), &targets).is_empty(),
                "go to {word} was underlined"
            );
        }
        assert!(defs.get("WS-AMT").is_some());
    }
}
