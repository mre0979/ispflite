// SPDX-License-Identifier: GPL-3.0-or-later
//! Line colorizer. Each column is one of a few kinds the window paints.
//! Block comments carry a flag from the previous line so `/* */` can span lines.

use std::path::Path;

pub const TEXT: u8 = 0;
pub const KEYWORD: u8 = 1;
pub const COMMENT: u8 = 2;
pub const STRING: u8 = 3;
pub const NUMBER: u8 = 4;
/// COPY and EXEC CICS, painted in the line-number color.
pub const DIRECTIVE: u8 = 5;
pub const COBOL_KEYWORD: u8 = 6;

/// Comment and statement state carried from the previous line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColorCarry {
    pub block: bool,
    pub copy: bool,
    pub cics: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    C,
    Hash,
    Dash,
    Basic,
    Markup,
    Json,
    Cobol,
    Plain,
}

/// Color one line. `in_block` is true when a `/*` comment is still open.
/// The bool is that flag for the next line.
pub fn color_line(text: &str, path: Option<&Path>, mut carry: ColorCarry) -> (Vec<u8>, ColorCarry) {
    let chars: Vec<char> = text.chars().collect();
    let family = family_of(path);
    if family == Family::Cobol {
        return color_cobol(&chars, carry);
    }
    let mut kinds = vec![TEXT; chars.len()];
    let mut i = 0;
    let mut in_block = carry.block && family == Family::C;
    while i < chars.len() {
        if in_block {
            let end = find_close(&chars, i);
            let stop = end.unwrap_or(chars.len());
            fill(&mut kinds, i, stop, COMMENT);
            if end.is_some() {
                in_block = false;
                i = stop;
            } else {
                break;
            }
            continue;
        }
        let ch = chars[i];
        if family == Family::C && starts_with(&chars, i, "/*") {
            in_block = true;
            continue;
        }
        if is_line_comment(family, &chars, i) {
            fill(&mut kinds, i, chars.len(), COMMENT);
            break;
        }
        if ch == '"' || ch == '\'' || (family == Family::Markup && ch == '`') {
            let quote = ch;
            kinds[i] = STRING;
            i += 1;
            while i < chars.len() {
                kinds[i] = STRING;
                if chars[i] == '\\' && family != Family::Basic && i + 1 < chars.len() {
                    i += 1;
                    kinds[i] = STRING;
                    i += 1;
                    continue;
                }
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if ch.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                kinds[i] = NUMBER;
                i += 1;
            }
            continue;
        }
        if is_ident_start(ch) {
            let start = i;
            i += 1;
            while i < chars.len() && is_ident_continue(chars[i]) {
                i += 1;
            }
            if is_keyword(family, &chars[start..i]) && !bordered_by_hyphen(&chars, start, i) {
                fill(&mut kinds, start, i, KEYWORD);
            }
            continue;
        }
        if family == Family::Markup && (ch == '<' || ch == '>' || ch == '/') {
            kinds[i] = KEYWORD;
        }
        i += 1;
    }
    carry.block = in_block;
    (kinds, carry)
}

fn family_of(path: Option<&Path>) -> Family {
    let ext = path
        .and_then(Path::extension)
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if crate::cobol::cobol_path(path) {
        return Family::Cobol;
    }
    match ext.as_str() {
        "rs" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "java" | "js" | "jsx" | "ts"
        | "tsx" | "cs" | "go" | "swift" | "kt" | "kts" | "scala" | "css" | "php" => Family::C,
        "py" | "pyw" | "sh" | "bash" | "zsh" | "rb" | "pl" | "r" | "toml" | "yaml" | "yml"
        | "cfg" | "ini" | "ps1" => Family::Hash,
        "sql" | "lua" | "hs" | "ada" | "adb" => Family::Dash,
        "bas" | "inc" | "pb" | "vb" => Family::Basic,
        "html" | "htm" | "xml" | "svg" => Family::Markup,
        "json" => Family::Json,
        _ => Family::Plain,
    }
}

fn is_line_comment(family: Family, chars: &[char], i: usize) -> bool {
    match family {
        Family::C | Family::Json => starts_with(chars, i, "//"),
        Family::Hash => chars[i] == '#',
        Family::Dash => starts_with(chars, i, "--"),
        Family::Basic => chars[i] == '\'' || starts_with_word(chars, i, "REM"),
        Family::Markup | Family::Plain | Family::Cobol => false,
    }
}

fn is_keyword(family: Family, word: &[char]) -> bool {
    let owned: String = word.iter().collect();
    let list: &[&str] = match family {
        Family::C => &[
            "as", "async", "await", "break", "case", "const", "continue", "default", "do", "else",
            "enum", "false", "fn", "for", "function", "if", "impl", "int", "let", "long", "match",
            "mod", "mut", "pub", "return", "self", "static", "struct", "switch", "trait", "true",
            "type", "use", "void", "while",
        ],
        Family::Hash => &[
            "and", "as", "class", "def", "elif", "else", "except", "false", "for", "from", "if",
            "import", "in", "is", "none", "not", "or", "pass", "return", "true", "try", "while",
            "with", "yield",
        ],
        Family::Dash => &[
            "and", "as", "by", "create", "delete", "from", "group", "insert", "into", "join",
            "left", "not", "on", "or", "order", "select", "set", "table", "update", "values",
            "where",
        ],
        Family::Basic => &[
            "and", "as", "case", "dim", "do", "else", "elseif", "end", "exit", "for", "function",
            "gosub", "goto", "if", "local", "long", "loop", "mod", "next", "not", "or", "return",
            "select", "step", "string", "sub", "then", "to", "wend", "while",
        ],
        Family::Json => &["false", "null", "true"],
        Family::Markup | Family::Plain | Family::Cobol => &[],
    };
    if family == Family::C {
        list.contains(&owned.as_str())
    } else {
        let lower = owned.to_ascii_lowercase();
        list.contains(&lower.as_str())
    }
}

fn starts_with(chars: &[char], i: usize, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    i + n.len() <= chars.len() && chars[i..i + n.len()] == n[..]
}

fn starts_with_word(chars: &[char], i: usize, word: &str) -> bool {
    let needle: Vec<char> = word.chars().collect();
    if i + needle.len() > chars.len() {
        return false;
    }
    let same = chars[i..i + needle.len()]
        .iter()
        .zip(&needle)
        .all(|(have, want)| have.eq_ignore_ascii_case(want));
    if !same {
        return false;
    }
    let end = i + needle.len();
    end == chars.len() || !is_ident_continue(chars[end])
}

fn find_close(chars: &[char], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == '*' && chars[i + 1] == '/' {
            return Some(i + 2);
        }
        i += 1;
    }
    None
}

fn fill(kinds: &mut [u8], start: usize, end: usize, kind: u8) {
    for slot in &mut kinds[start..end] {
        *slot = kind;
    }
}

/// A hyphen on either side makes the word a name, not a keyword.
fn bordered_by_hyphen(chars: &[char], start: usize, end: usize) -> bool {
    (start > 0 && chars[start - 1] == '-') || (end < chars.len() && chars[end] == '-')
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn color_cobol(chars: &[char], mut carry: ColorCarry) -> (Vec<u8>, ColorCarry) {
    let mut kinds = vec![TEXT; chars.len()];
    if cobol_comment_line(chars) {
        fill(&mut kinds, 0, chars.len(), COMMENT);
        return (kinds, carry);
    }
    let mut i = 0;
    if carry.cics {
        match scan_region(chars, 0, Region::Cics) {
            Stop::Done(end) => {
                fill(&mut kinds, 0, end, DIRECTIVE);
                carry.cics = false;
                i = end;
            }
            Stop::Comment(at) => {
                fill(&mut kinds, 0, at, DIRECTIVE);
                fill(&mut kinds, at, chars.len(), COMMENT);
                return (kinds, carry);
            }
            Stop::Continues => {
                fill(&mut kinds, 0, chars.len(), DIRECTIVE);
                return (kinds, carry);
            }
        }
    } else if carry.copy {
        match scan_region(chars, 0, Region::Copy) {
            Stop::Done(end) => {
                fill(&mut kinds, 0, end, DIRECTIVE);
                carry.copy = false;
                i = end;
            }
            Stop::Comment(at) => {
                fill(&mut kinds, 0, at, DIRECTIVE);
                fill(&mut kinds, at, chars.len(), COMMENT);
                return (kinds, carry);
            }
            Stop::Continues => {
                fill(&mut kinds, 0, chars.len(), DIRECTIVE);
                return (kinds, carry);
            }
        }
    }
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '*' && i + 1 < chars.len() && chars[i + 1] == '>' {
            fill(&mut kinds, i, chars.len(), COMMENT);
            break;
        }
        if ch == '"' || ch == '\'' {
            i = paint_cobol_string(&mut kinds, chars, i);
            continue;
        }
        if ch.is_ascii_digit() {
            kinds[i] = NUMBER;
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                kinds[i] = NUMBER;
                i += 1;
            }
            if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                kinds[i] = NUMBER;
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    kinds[i] = NUMBER;
                    i += 1;
                }
            }
            continue;
        }
        if ch.is_ascii_alphabetic() {
            let start = i;
            let (word, next) = take_cobol_word(chars, i);
            if word.eq_ignore_ascii_case("COPY") {
                match scan_region(chars, start, Region::Copy) {
                    Stop::Done(end) => {
                        fill(&mut kinds, start, end, DIRECTIVE);
                        i = end;
                    }
                    Stop::Comment(at) => {
                        fill(&mut kinds, start, at, DIRECTIVE);
                        fill(&mut kinds, at, chars.len(), COMMENT);
                        carry.copy = true;
                        break;
                    }
                    Stop::Continues => {
                        fill(&mut kinds, start, chars.len(), DIRECTIVE);
                        carry.copy = true;
                        break;
                    }
                }
                continue;
            }
            if word.eq_ignore_ascii_case("EXEC")
                && next_cobol_word(chars, next)
                    .is_some_and(|(_, _, next_word)| next_word.eq_ignore_ascii_case("CICS"))
            {
                match scan_region(chars, start, Region::Cics) {
                    Stop::Done(end) => {
                        fill(&mut kinds, start, end, DIRECTIVE);
                        i = end;
                    }
                    Stop::Comment(at) => {
                        fill(&mut kinds, start, at, DIRECTIVE);
                        fill(&mut kinds, at, chars.len(), COMMENT);
                        carry.cics = true;
                        break;
                    }
                    Stop::Continues => {
                        fill(&mut kinds, start, chars.len(), DIRECTIVE);
                        carry.cics = true;
                        break;
                    }
                }
                continue;
            }
            if cobol_keyword(&word) && !bordered_by_hyphen(chars, start, next) {
                fill(&mut kinds, start, next, COBOL_KEYWORD);
            }
            i = next;
            continue;
        }
        i += 1;
    }
    (kinds, carry)
}

fn cobol_comment_line(chars: &[char]) -> bool {
    if chars.len() >= 7 {
        let digits = chars[..6].iter().all(|ch| ch.is_ascii_digit());
        let blank = chars[..6].iter().all(|ch| *ch == ' ');
        if (digits || blank) && matches!(chars[6], '*' | '/') {
            return true;
        }
    }
    let start = chars.iter().position(|ch| !ch.is_whitespace()).unwrap_or(0);
    start < chars.len() && chars[start] == '*'
}

fn paint_cobol_string(kinds: &mut [u8], chars: &[char], start: usize) -> usize {
    let quote = chars[start];
    kinds[start] = STRING;
    let mut i = start + 1;
    while i < chars.len() {
        kinds[i] = STRING;
        if chars[i] == quote {
            if i + 1 < chars.len() && chars[i + 1] == quote {
                kinds[i + 1] = STRING;
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

fn take_cobol_word(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start + 1;
    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
        i += 1;
    }
    while i > start + 1 && chars[i - 1] == '-' {
        i -= 1;
    }
    (chars[start..i].iter().collect(), i)
}

fn next_cobol_word(chars: &[char], mut i: usize) -> Option<(usize, usize, String)> {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if i >= chars.len() || !chars[i].is_ascii_alphabetic() {
        return None;
    }
    let (word, end) = take_cobol_word(chars, i);
    Some((i, end, word))
}

enum Region {
    Copy,
    Cics,
}

enum Stop {
    Done(usize),
    Comment(usize),
    Continues,
}

fn scan_region(chars: &[char], from: usize, region: Region) -> Stop {
    let mut i = from;
    let mut quote: Option<char> = None;
    let mut pseudo = false;
    while i < chars.len() {
        let ch = chars[i];
        if let Some(q) = quote {
            if ch == q {
                if i + 1 < chars.len() && chars[i + 1] == q {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == '*' && i + 1 < chars.len() && chars[i + 1] == '>' {
            return Stop::Comment(i);
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if ch == '=' && i + 1 < chars.len() && chars[i + 1] == '=' {
            pseudo = !pseudo;
            i += 2;
            continue;
        }
        if matches!(region, Region::Copy) && ch == '.' && !pseudo {
            return Stop::Done(i + 1);
        }
        if matches!(region, Region::Cics) && ch.is_ascii_alphabetic() {
            let (word, end) = take_cobol_word(chars, i);
            if word.eq_ignore_ascii_case("END-EXEC") {
                return Stop::Done(end);
            }
            i = end;
            continue;
        }
        i += 1;
    }
    Stop::Continues
}

pub(crate) fn cobol_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    COBOL_KEYWORDS.binary_search(&upper.as_str()).is_ok()
}

#[cfg(test)]
pub(crate) fn cobol_keywords() -> &'static [&'static str] {
    COBOL_KEYWORDS
}

const COBOL_KEYWORDS: &[&str] = &[
    "ACCEPT",
    "ACCESS",
    "ADD",
    "ADDRESS",
    "ADVANCING",
    "AFTER",
    "ALL",
    "ALLOCATE",
    "ALPHABET",
    "ALPHABETIC",
    "ALPHABETIC-LOWER",
    "ALPHABETIC-UPPER",
    "ALPHANUMERIC",
    "ALPHANUMERIC-EDITED",
    "ALSO",
    "ALTER",
    "ALTERNATE",
    "AND",
    "ANY",
    "ARE",
    "AREA",
    "AREAS",
    "ASCENDING",
    "ASSIGN",
    "AT",
    "AUTHOR",
    "BEFORE",
    "BINARY",
    "BLANK",
    "BLOCK",
    "BOTTOM",
    "BY",
    "CALL",
    "CANCEL",
    "CD",
    "CF",
    "CH",
    "CHARACTER",
    "CHARACTERS",
    "CLASS",
    "CLOCK-UNITS",
    "CLOSE",
    "COBOL",
    "CODE",
    "CODE-SET",
    "COLLATING",
    "COLUMN",
    "COMMA",
    "COMMON",
    "COMMUNICATION",
    "COMP",
    "COMP-1",
    "COMP-2",
    "COMP-3",
    "COMP-4",
    "COMP-5",
    "COMPUTATIONAL",
    "COMPUTATIONAL-1",
    "COMPUTATIONAL-2",
    "COMPUTATIONAL-3",
    "COMPUTATIONAL-4",
    "COMPUTATIONAL-5",
    "COMPUTE",
    "CONFIGURATION",
    "CONTAINS",
    "CONTENT",
    "CONTINUE",
    "CONTROL",
    "CONTROLS",
    "CONVERTING",
    "COPY",
    "CORR",
    "CORRESPONDING",
    "COUNT",
    "CURRENCY",
    "DATA",
    "DATE",
    "DATE-COMPILED",
    "DATE-WRITTEN",
    "DAY",
    "DAY-OF-WEEK",
    "DE",
    "DEBUG-CONTENTS",
    "DEBUG-ITEM",
    "DEBUG-LINE",
    "DEBUG-NAME",
    "DEBUG-SUB-1",
    "DEBUG-SUB-2",
    "DEBUG-SUB-3",
    "DEBUGGING",
    "DECIMAL-POINT",
    "DECLARATIVES",
    "DELETE",
    "DELIMITED",
    "DELIMITER",
    "DEPENDING",
    "DESCENDING",
    "DESTINATION",
    "DETAIL",
    "DISABLE",
    "DISPLAY",
    "DIVIDE",
    "DIVISION",
    "DOWN",
    "DUPLICATES",
    "DYNAMIC",
    "EGCS",
    "EGI",
    "ELSE",
    "EMI",
    "ENABLE",
    "END",
    "END-ADD",
    "END-CALL",
    "END-COMPUTE",
    "END-DELETE",
    "END-DIVIDE",
    "END-EVALUATE",
    "END-EXEC",
    "END-IF",
    "END-MULTIPLY",
    "END-OF-PAGE",
    "END-PERFORM",
    "END-READ",
    "END-RECEIVE",
    "END-RETURN",
    "END-REWRITE",
    "END-SEARCH",
    "END-START",
    "END-STRING",
    "END-SUBTRACT",
    "END-UNSTRING",
    "END-WRITE",
    "ENTER",
    "ENTRY",
    "ENVIRONMENT",
    "EOP",
    "EQUAL",
    "ERROR",
    "ESI",
    "EVALUATE",
    "EVERY",
    "EXCEPTION",
    "EXEC",
    "EXIT",
    "EXTEND",
    "EXTERNAL",
    "FALSE",
    "FD",
    "FILE",
    "FILE-CONTROL",
    "FILLER",
    "FINAL",
    "FIRST",
    "FOOTING",
    "FOR",
    "FREE",
    "FROM",
    "FUNCTION",
    "FUNCTION-POINTER",
    "GENERATE",
    "GIVING",
    "GLOBAL",
    "GO",
    "GOBACK",
    "GREATER",
    "GROUP",
    "HEADING",
    "HIGH-VALUE",
    "HIGH-VALUES",
    "I-O",
    "I-O-CONTROL",
    "IDENTIFICATION",
    "IF",
    "IN",
    "INDEX",
    "INDEXED",
    "INDICATE",
    "INITIAL",
    "INITIALIZE",
    "INITIATE",
    "INPUT",
    "INPUT-OUTPUT",
    "INSPECT",
    "INSTALLATION",
    "INTO",
    "INVALID",
    "IS",
    "JSON",
    "JUST",
    "JUSTIFIED",
    "KEY",
    "LABEL",
    "LAST",
    "LEADING",
    "LEFT",
    "LENGTH",
    "LESS",
    "LIMIT",
    "LIMITS",
    "LINAGE",
    "LINAGE-COUNTER",
    "LINE",
    "LINE-COUNTER",
    "LINES",
    "LINKAGE",
    "LOCAL-STORAGE",
    "LOCK",
    "LOW-VALUE",
    "LOW-VALUES",
    "MEMORY",
    "MERGE",
    "MESSAGE",
    "MODE",
    "MODULES",
    "MOVE",
    "MULTIPLE",
    "MULTIPLY",
    "NATIONAL",
    "NATIONAL-EDITED",
    "NATIVE",
    "NEGATIVE",
    "NEXT",
    "NO",
    "NOT",
    "NULL",
    "NULLS",
    "NUMBER",
    "NUMERIC",
    "NUMERIC-EDITED",
    "OBJECT-COMPUTER",
    "OCCURS",
    "OF",
    "OFF",
    "OMITTED",
    "ON",
    "OPEN",
    "OPTIONAL",
    "OR",
    "ORDER",
    "ORGANIZATION",
    "OTHER",
    "OUTPUT",
    "OVERFLOW",
    "PACKED-DECIMAL",
    "PADDING",
    "PAGE",
    "PAGE-COUNTER",
    "PERFORM",
    "PF",
    "PH",
    "PIC",
    "PICTURE",
    "PLUS",
    "POINTER",
    "POSITION",
    "POSITIVE",
    "PRINTING",
    "PROCEDURE",
    "PROCEDURE-POINTER",
    "PROCEDURES",
    "PROCEED",
    "PROGRAM",
    "PROGRAM-ID",
    "PURGE",
    "QUEUE",
    "QUOTE",
    "QUOTES",
    "RANDOM",
    "RD",
    "READ",
    "RECEIVE",
    "RECORD",
    "RECORDING",
    "RECORDS",
    "REDEFINES",
    "REEL",
    "REFERENCE",
    "REFERENCES",
    "RELATIVE",
    "RELEASE",
    "REMAINDER",
    "REMOVAL",
    "RENAMES",
    "REPLACE",
    "REPLACING",
    "REPORT",
    "REPORTING",
    "REPORTS",
    "REPOSITORY",
    "RERUN",
    "RESERVE",
    "RESET",
    "RETURN",
    "RETURNING",
    "REVERSED",
    "REWIND",
    "REWRITE",
    "RF",
    "RH",
    "RIGHT",
    "ROUNDED",
    "RUN",
    "SAME",
    "SD",
    "SEARCH",
    "SECTION",
    "SECURITY",
    "SEGMENT",
    "SEGMENT-LIMIT",
    "SELECT",
    "SEND",
    "SENTENCE",
    "SEPARATE",
    "SEQUENCE",
    "SEQUENTIAL",
    "SET",
    "SIGN",
    "SIZE",
    "SORT",
    "SORT-MERGE",
    "SOURCE",
    "SOURCE-COMPUTER",
    "SPACE",
    "SPACES",
    "SPECIAL-NAMES",
    "SQL",
    "STANDARD",
    "STANDARD-1",
    "STANDARD-2",
    "START",
    "STATUS",
    "STOP",
    "STRING",
    "SUB-QUEUE-1",
    "SUB-QUEUE-2",
    "SUB-QUEUE-3",
    "SUBTRACT",
    "SUM",
    "SUPPRESS",
    "SYMBOLIC",
    "SYNC",
    "SYNCHRONIZED",
    "TABLE",
    "TALLYING",
    "TAPE",
    "TERMINAL",
    "TERMINATE",
    "TEST",
    "TEXT",
    "THAN",
    "THEN",
    "THROUGH",
    "THRU",
    "TIME",
    "TIMES",
    "TO",
    "TOP",
    "TRAILING",
    "TRUE",
    "TYPE",
    "UNIT",
    "UNSTRING",
    "UNTIL",
    "UP",
    "UPON",
    "USAGE",
    "USE",
    "USING",
    "VALIDATE",
    "VALUE",
    "VALUES",
    "VARYING",
    "WHEN",
    "WHEN-COMPILED",
    "WITH",
    "WORDS",
    "WORKING-STORAGE",
    "WRITE",
    "XML",
    "ZERO",
    "ZEROES",
    "ZEROS",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keywords_comments_and_strings() {
        let (kinds, open) = color_line(
            "fn main() { let s = \"hi\"; // tail",
            Some(Path::new("a.rs")),
            ColorCarry::default(),
        );
        assert!(!open.block);
        assert_eq!(kinds[0], KEYWORD);
        assert_eq!(kinds[3], TEXT);
        let quote = "fn main() { let s = ".chars().count();
        assert_eq!(kinds[quote], STRING);
        let slash = kinds.iter().rposition(|k| *k == COMMENT).unwrap();
        assert_eq!(kinds[slash], COMMENT);
    }

    #[test]
    fn block_comment_crosses_the_line() {
        let (_, open) = color_line(
            "before /* start",
            Some(Path::new("a.c")),
            ColorCarry::default(),
        );
        assert!(open.block);
        let (kinds, still) = color_line("still * / end", Some(Path::new("a.c")), open);
        assert!(still.block);
        assert!(kinds.iter().all(|k| *k == COMMENT));
        let (_, closed) = color_line("end */ after", Some(Path::new("a.c")), open);
        assert!(!closed.block);
    }

    #[test]
    fn cobol_keywords_are_sorted() {
        for pair in COBOL_KEYWORDS.windows(2) {
            assert!(pair[0] < pair[1], "{} then {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn cobol_colors_comments_keywords_numbers_copy_and_cics() {
        let comment = color_line(
            "000100* this is a comment",
            Some(Path::new("a.cbl")),
            ColorCarry::default(),
        )
        .0;
        assert!(comment.iter().all(|kind| *kind == COMMENT));

        let inline = color_line(
            "    DISPLAY 'X'. *> tail",
            Some(Path::new("a.cbl")),
            ColorCarry::default(),
        )
        .0;
        assert_eq!(inline[4], COBOL_KEYWORD);
        assert!(inline.iter().any(|kind| *kind == STRING));
        assert_eq!(*inline.last().unwrap(), COMMENT);

        let moved = color_line(
            "    MOVE 12 TO WS-COUNT.",
            Some(Path::new("a.cbl")),
            ColorCarry::default(),
        )
        .0;
        assert_eq!(moved[4], COBOL_KEYWORD);
        assert_eq!(moved[9], NUMBER);
        assert_eq!(moved[12], COBOL_KEYWORD);
        assert_eq!(moved[15], TEXT);

        let joined = color_line(
            "    MOVE-ITEM END-IF -MOVE MOVE-",
            Some(Path::new("a.cbl")),
            ColorCarry::default(),
        )
        .0;
        assert!(joined[4..13].iter().all(|kind| *kind == TEXT), "{joined:?}");
        assert!(
            joined[14..20].iter().all(|kind| *kind == COBOL_KEYWORD),
            "{joined:?}"
        );
        assert!(joined[22..].iter().all(|kind| *kind == TEXT), "{joined:?}");

        let named = color_line(
            "let self-type = pre-if",
            Some(Path::new("a.rs")),
            ColorCarry::default(),
        )
        .0;
        assert_eq!(named[0], KEYWORD);
        assert!(named[4..].iter().all(|kind| *kind == TEXT), "{named:?}");

        let copy = color_line(
            "    COPY CUSTMAST.",
            Some(Path::new("a.cpy")),
            ColorCarry::default(),
        )
        .0;
        assert!(copy[4..].iter().all(|kind| *kind == DIRECTIVE));

        let (open, carry) = color_line(
            "    EXEC CICS",
            Some(Path::new("a.cbl")),
            ColorCarry::default(),
        );
        assert!(carry.cics);
        assert!(open[4..].iter().all(|kind| *kind == DIRECTIVE));
        let (body, carry) = color_line("        RETURN", Some(Path::new("a.cbl")), carry);
        assert!(carry.cics);
        assert!(body.iter().all(|kind| *kind == DIRECTIVE));
        let (end, carry) = color_line("    END-EXEC.", Some(Path::new("a.cbl")), carry);
        assert!(!carry.cics);
        assert_eq!(end[4], DIRECTIVE);
        assert_eq!(*end.last().unwrap(), TEXT);
    }
}
