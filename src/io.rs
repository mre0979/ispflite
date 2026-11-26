// SPDX-License-Identifier: GPL-3.0-or-later
//! File bytes in and out. New files are UTF-8. Bytes that are not UTF-8 are
//! read as Windows-1252, which is the ANSI encoding used for older files.

/// How line text is stored on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Windows1252,
}

impl Encoding {
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_uppercase().as_str() {
            "UTF8" | "UTF-8" => Some(Self::Utf8),
            "UTF8BOM" | "UTF-8-BOM" | "BOM" => Some(Self::Utf8Bom),
            "ANSI" | "WIN1252" | "WINDOWS-1252" | "CP1252" => Some(Self::Windows1252),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 BOM",
            Self::Windows1252 => "Windows-1252",
        }
    }
}

/// Line ending preserved from the file that was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newline {
    Lf,
    Crlf,
}

impl Newline {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }

    pub fn platform() -> Self {
        if cfg!(windows) {
            Self::Crlf
        } else {
            Self::Lf
        }
    }
}

/// Unicode mapping for Windows-1252 bytes 0x80..=0x9F.
const WIN1252_80: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

pub(crate) fn decode_1252_byte(b: u8) -> char {
    if (0x80..0xA0).contains(&b) {
        WIN1252_80[(b - 0x80) as usize]
    } else {
        char::from(b)
    }
}

pub(crate) fn encode_1252_char(c: char) -> Option<u8> {
    match c {
        '\u{20AC}' => Some(0x80),
        '\u{201A}' => Some(0x82),
        '\u{0192}' => Some(0x83),
        '\u{201E}' => Some(0x84),
        '\u{2026}' => Some(0x85),
        '\u{2020}' => Some(0x86),
        '\u{2021}' => Some(0x87),
        '\u{02C6}' => Some(0x88),
        '\u{2030}' => Some(0x89),
        '\u{0160}' => Some(0x8A),
        '\u{2039}' => Some(0x8B),
        '\u{0152}' => Some(0x8C),
        '\u{017D}' => Some(0x8E),
        '\u{2018}' => Some(0x91),
        '\u{2019}' => Some(0x92),
        '\u{201C}' => Some(0x93),
        '\u{201D}' => Some(0x94),
        '\u{2022}' => Some(0x95),
        '\u{2013}' => Some(0x96),
        '\u{2014}' => Some(0x97),
        '\u{02DC}' => Some(0x98),
        '\u{2122}' => Some(0x99),
        '\u{0161}' => Some(0x9A),
        '\u{203A}' => Some(0x9B),
        '\u{0153}' => Some(0x9C),
        '\u{017E}' => Some(0x9E),
        '\u{0178}' => Some(0x9F),
        c if (c as u32) < 0x80 => Some(c as u8),
        c if (0xA0..=0xFF).contains(&(c as u32)) => Some(c as u8),
        c if (0x80..0xA0).contains(&(c as u32)) && decode_1252_byte(c as u8) == c => Some(c as u8),
        _ => None,
    }
}

pub(crate) struct Decoded {
    pub text: String,
    pub encoding: Encoding,
}

pub(crate) fn decode_bytes(bytes: &[u8]) -> Decoded {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Decoded {
            text: String::from_utf8_lossy(rest).into_owned(),
            encoding: Encoding::Utf8Bom,
        };
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Decoded {
            text: text.to_string(),
            encoding: Encoding::Utf8,
        };
    }
    Decoded {
        text: bytes.iter().copied().map(decode_1252_byte).collect(),
        encoding: Encoding::Windows1252,
    }
}

pub(crate) fn encode_text(text: &str, encoding: Encoding) -> Result<Vec<u8>, char> {
    match encoding {
        Encoding::Utf8 => Ok(text.as_bytes().to_vec()),
        Encoding::Utf8Bom => {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(text.as_bytes());
            Ok(out)
        }
        Encoding::Windows1252 => {
            let mut out = Vec::with_capacity(text.len());
            for c in text.chars() {
                out.push(encode_1252_char(c).ok_or(c)?);
            }
            Ok(out)
        }
    }
}

/// Split file text into lines and report whether it ended with a newline.
///
/// `""` is an empty file. `"\n"` is one empty line. A trailing newline does
/// not create an extra line.
pub(crate) fn split_lines(text: &str) -> (Vec<String>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let trailing = text.ends_with('\n');
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    if trimmed.is_empty() {
        return (vec![String::new()], trailing);
    }
    let lines = trimmed
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    (lines, trailing)
}

pub(crate) fn join_lines(lines: &[String], newline: Newline, trailing: bool) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join(newline.as_str());
    if trailing {
        text.push_str(newline.as_str());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_utf8_and_1252() {
        let euro = decode_bytes(&[0x80, b'A']);
        assert_eq!(euro.encoding, Encoding::Windows1252);
        assert_eq!(euro.text, "€A");
        assert_eq!(
            encode_text(&euro.text, Encoding::Windows1252).unwrap(),
            vec![0x80, b'A']
        );

        let utf = decode_bytes("café\n".as_bytes());
        assert_eq!(utf.encoding, Encoding::Utf8);
        assert_eq!(utf.text, "café\n");
    }

    #[test]
    fn split_keeps_blank_lines_and_trailing_flag() {
        assert_eq!(split_lines(""), (Vec::<String>::new(), false));
        assert_eq!(split_lines("\n"), (vec![String::new()], true));
        assert_eq!(
            split_lines("\n\n"),
            (vec![String::new(), String::new()], true)
        );
        assert_eq!(split_lines("a\nb"), (vec!["a".into(), "b".into()], false));
        assert_eq!(
            split_lines("a\r\nb\r\n"),
            (vec!["a".into(), "b".into()], true)
        );
    }
}
