//! Edit one `KEY=value` line of a shell env file (`squeezelite.env`,
//! `mestro.env`: sourced by the init scripts, editable on any OS
//! because the data partition is FAT). Everything else in the file
//! survives byte for byte — comments, other keys, quoting, line
//! endings; the write is atomic, so a reader (the init script, a
//! user over SMB) never sees a truncated file.

use std::path::Path;

/// one parsed `[export] KEY=word [# comment]` line, split so the
/// pieces around the value can be written back untouched
struct Assignment<'a> {
    key: &'a str,
    /// everything up to and including the `=` (indent, `export`, BOM)
    head: &'a str,
    /// the raw shell word after the `=`, quotes included
    word: &'a str,
    /// what follows the word (blanks, a trailing comment)
    tail: &'a str,
}

/// how the original value was written — kept on rewrite
#[derive(Clone, Copy, PartialEq)]
enum Quoting {
    Bare,
    Single,
    Double,
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// a line (without its terminator) as an assignment of any key;
/// comments, blank lines and everything else are None
fn parse_line(content: &str) -> Option<Assignment<'_>> {
    // an editor on Windows may prepend a BOM; it must not hide the
    // first key (the shell does see it as part of the name, but
    // the user means the key)
    let bom = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let body = content.get(bom..)?.trim_start_matches(is_blank);
    let body = match body.strip_prefix("export") {
        Some(rest) if rest.starts_with(is_blank) => rest.trim_start_matches(is_blank),
        _ => body,
    };
    let key_len = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(body.len());
    let key = body.get(..key_len)?;
    if !is_valid_key(key) {
        return None;
    }
    let value_part = body.get(key_len..)?.strip_prefix('=')?;
    let head = content.get(..content.len() - value_part.len())?;
    let word_len = word_end(value_part);
    Some(Assignment {
        key,
        head,
        word: value_part.get(..word_len)?,
        tail: value_part.get(word_len..)?,
    })
}

/// the byte length of the leading shell word: up to the first
/// unquoted blank
fn word_end(text: &str) -> usize {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(open) = quote {
            if c == open {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if is_blank(c) {
            return index;
        }
    }
    text.len()
}

/// the value a shell word stands for
fn unquote(word: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (Some('\''), c) => out.push(c),
            (_, '\\') => match chars.next() {
                Some(next @ ('"' | '\\' | '$' | '`')) => out.push(next),
                Some(next) if quote.is_none() => out.push(next),
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push('\\'),
            },
            (_, c) => out.push(c),
        }
    }
    out
}

fn quoting_of(word: &str) -> Quoting {
    match word.chars().next() {
        Some('"') => Quoting::Double,
        Some('\'') => Quoting::Single,
        _ => Quoting::Bare,
    }
}

/// characters that need no quoting in a shell word
fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)
}

/// the shell word for `value`, in the original's quoting style
/// where that can hold the value
fn render(value: &str, style: Quoting) -> String {
    match style {
        Quoting::Single if !value.contains('\'') => format!("'{value}'"),
        Quoting::Bare if value.chars().all(is_plain) => value.to_string(),
        _ => {
            let mut word = String::from('"');
            for c in value.chars() {
                if matches!(c, '"' | '\\' | '$' | '`') {
                    word.push('\\');
                }
                word.push(c);
            }
            word.push('"');
            word
        }
    }
}

/// one line split into its content and its terminator (`\n`, `\r\n`
/// or none on the last line)
fn split_terminator(line: &str) -> (&str, &str) {
    let content = line.strip_suffix('\n').unwrap_or(line);
    let content = content.strip_suffix('\r').unwrap_or(content);
    (content, line.get(content.len()..).unwrap_or_default())
}

/// set `key` to `value` in `text`: the last assignment of the key
/// is rewritten in place (the one the shell ends up with), a
/// missing key is appended. Returns the new text.
pub fn set_key(text: &str, key: &str, value: &str) -> Result<String, String> {
    if !is_valid_key(key) {
        return Err(format!("{key:?} is not a variable name"));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err("a value cannot hold a line break".into());
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let last = lines
        .iter()
        .rposition(|line| parse_line(split_terminator(line).0).is_some_and(|a| a.key == key));
    let mut out = String::with_capacity(text.len() + key.len() + value.len() + 4);
    for (index, line) in lines.iter().enumerate() {
        let (content, terminator) = split_terminator(line);
        match parse_line(content).filter(|_| Some(index) == last) {
            Some(found) => {
                out.push_str(found.head);
                out.push_str(&render(value, quoting_of(found.word)));
                out.push_str(found.tail);
                out.push_str(terminator);
            }
            None => out.push_str(line),
        }
    }
    if last.is_none() {
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(&format!("{key}={}{eol}", render(value, Quoting::Bare)));
    }
    Ok(out)
}

/// the value the shell would see for `key`: the last assignment,
/// unquoted. None when the key is not assigned
pub fn get_key(text: &str, key: &str) -> Option<String> {
    text.split_inclusive('\n')
        .filter_map(|line| parse_line(split_terminator(line).0))
        .rfind(|assignment| assignment.key == key)
        .map(|assignment| unquote(assignment.word))
}

/// [`set_key`] on a file, written atomically. A missing file is
/// created; a file that is not UTF-8 is refused untouched. An
/// edit that changes nothing does not write at all
pub fn set_key_in_file(path: &Path, key: &str, value: &str) -> Result<(), String> {
    let (old, existed) = match std::fs::read(path) {
        Ok(bytes) => (
            String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?,
            true,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let new = set_key(&old, key, value)?;
    if existed && new == old {
        return Ok(());
    }
    crate::atomic_file::write_atomic(path, &new)
}

/// [`get_key`] on a file; None when the file or the key is missing
pub fn get_key_in_file(path: &Path, key: &str) -> Option<String> {
    get_key(&std::fs::read_to_string(path).ok()?, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUEEZELITE: &str = "# squeezelite env\n\nOUTPUT=hw:CARD=DAC\n# Music Assistant host\nSERVER=\nNAME=mestro\nEXTRA=\"\"\n";

    #[test]
    fn replaces_only_the_one_line_and_keeps_every_comment() {
        let out = set_key(SQUEEZELITE, "OUTPUT", "hw:CARD=S3").unwrap();
        assert_eq!(
            out,
            "# squeezelite env\n\nOUTPUT=hw:CARD=S3\n# Music Assistant host\nSERVER=\nNAME=mestro\nEXTRA=\"\"\n"
        );
    }

    #[test]
    fn crlf_line_endings_survive_and_are_used_for_appends() {
        let text = "# c\r\nOUTPUT=hw:CARD=DAC\r\nNAME=mestro\r\n";
        assert_eq!(
            set_key(text, "OUTPUT", "hw:CARD=S3").unwrap(),
            "# c\r\nOUTPUT=hw:CARD=S3\r\nNAME=mestro\r\n"
        );
        assert_eq!(
            set_key(text, "TZ", "Europe/Berlin").unwrap(),
            "# c\r\nOUTPUT=hw:CARD=DAC\r\nNAME=mestro\r\nTZ=Europe/Berlin\r\n"
        );
    }

    #[test]
    fn mixed_endings_keep_each_lines_own_terminator() {
        let text = "A=1\nOUTPUT=x\r\nB=2\n";
        assert_eq!(
            set_key(text, "OUTPUT", "y").unwrap(),
            "A=1\nOUTPUT=y\r\nB=2\n"
        );
    }

    #[test]
    fn a_missing_key_is_appended_after_the_last_line() {
        assert_eq!(
            set_key("A=1\n", "TZ", "Europe/Berlin").unwrap(),
            "A=1\nTZ=Europe/Berlin\n"
        );
        // no trailing newline: one is added before the new line
        assert_eq!(
            set_key("A=1", "TZ", "Europe/Berlin").unwrap(),
            "A=1\nTZ=Europe/Berlin\n"
        );
        assert_eq!(set_key("", "TZ", "UTC").unwrap(), "TZ=UTC\n");
    }

    #[test]
    fn a_commented_assignment_is_not_the_key() {
        let text = "#OUTPUT=hw:CARD=OLD\n  # OUTPUT=hw:CARD=OLD2\n";
        assert_eq!(
            set_key(text, "OUTPUT", "hw:CARD=S3").unwrap(),
            "#OUTPUT=hw:CARD=OLD\n  # OUTPUT=hw:CARD=OLD2\nOUTPUT=hw:CARD=S3\n"
        );
        assert_eq!(get_key(text, "OUTPUT"), None);
    }

    #[test]
    fn a_key_that_only_shares_a_prefix_is_left_alone() {
        let text = "OUTPUT_MODE=a\nXOUTPUT=b\n";
        assert_eq!(
            set_key(text, "OUTPUT", "c").unwrap(),
            "OUTPUT_MODE=a\nXOUTPUT=b\nOUTPUT=c\n"
        );
    }

    #[test]
    fn quoting_style_and_trailing_comments_are_kept() {
        assert_eq!(
            set_key("OUTPUT=\"hw:CARD=DAC\"\n", "OUTPUT", "hw:CARD=S3").unwrap(),
            "OUTPUT=\"hw:CARD=S3\"\n"
        );
        assert_eq!(
            set_key("OUTPUT='hw:CARD=DAC'\n", "OUTPUT", "hw:CARD=S3").unwrap(),
            "OUTPUT='hw:CARD=S3'\n"
        );
        assert_eq!(
            set_key(
                "OUTPUT=hw:CARD=DAC   # bit-perfect\n",
                "OUTPUT",
                "hw:CARD=S3"
            )
            .unwrap(),
            "OUTPUT=hw:CARD=S3   # bit-perfect\n"
        );
        assert_eq!(
            set_key("OUTPUT=\"a b\" # why\n", "OUTPUT", "c").unwrap(),
            "OUTPUT=\"c\" # why\n"
        );
    }

    #[test]
    fn export_prefix_and_indent_are_kept() {
        assert_eq!(
            set_key("  export TZ=UTC\n", "TZ", "Europe/Berlin").unwrap(),
            "  export TZ=Europe/Berlin\n"
        );
    }

    #[test]
    fn values_that_need_quoting_get_quotes() {
        assert_eq!(
            set_key("", "NAME", "my player").unwrap(),
            "NAME=\"my player\"\n"
        );
        // shell metacharacters stay inert inside the quotes
        assert_eq!(
            set_key("", "NAME", "a\"b$c`d\\e").unwrap(),
            "NAME=\"a\\\"b\\$c\\`d\\\\e\"\n"
        );
        // a single-quoted original cannot hold a quote: switch style
        assert_eq!(
            set_key("NAME='x'\n", "NAME", "it's").unwrap(),
            "NAME=\"it's\"\n"
        );
        assert_eq!(set_key("", "EXTRA", "").unwrap(), "EXTRA=\n");
    }

    #[test]
    fn the_last_assignment_wins_like_in_the_shell() {
        let text = "OUTPUT=a\nNAME=n\nOUTPUT=b\n";
        assert_eq!(get_key(text, "OUTPUT").as_deref(), Some("b"));
        assert_eq!(
            set_key(text, "OUTPUT", "c").unwrap(),
            "OUTPUT=a\nNAME=n\nOUTPUT=c\n"
        );
    }

    #[test]
    fn get_key_unquotes_and_drops_comments() {
        assert_eq!(get_key("A=\"x y\" # c\n", "A").as_deref(), Some("x y"));
        assert_eq!(get_key("A='x y'\n", "A").as_deref(), Some("x y"));
        assert_eq!(get_key("A=x # c\n", "A").as_deref(), Some("x"));
        assert_eq!(
            get_key("A=\"a\\\"b\\$c\"\n", "A").as_deref(),
            Some("a\"b$c")
        );
        assert_eq!(get_key("A=\r\n", "A").as_deref(), Some(""));
        assert_eq!(get_key("B=1\n", "A"), None);
    }

    #[test]
    fn a_utf8_bom_from_windows_notepad_is_kept_and_does_not_hide_the_first_key() {
        let text = "\u{feff}OUTPUT=a\nNAME=n\n";
        assert_eq!(get_key(text, "OUTPUT").as_deref(), Some("a"));
        assert_eq!(
            set_key(text, "OUTPUT", "b").unwrap(),
            "\u{feff}OUTPUT=b\nNAME=n\n"
        );
    }

    #[test]
    fn a_bad_key_or_a_multiline_value_is_refused() {
        for key in ["", "1A", "A B", "A=B", "A\n"] {
            assert!(set_key("", key, "x").is_err(), "{key:?}");
        }
        for value in ["a\nb", "a\rb", "a\0b"] {
            assert!(set_key("A=1\n", "A", value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn the_file_edit_is_atomic_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("squeezelite.env");
        std::fs::write(&file, SQUEEZELITE).unwrap();
        set_key_in_file(&file, "OUTPUT", "hw:CARD=S3").unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            SQUEEZELITE.replace("hw:CARD=DAC", "hw:CARD=S3")
        );
        assert_eq!(
            get_key_in_file(&file, "OUTPUT").as_deref(),
            Some("hw:CARD=S3")
        );
        // no scratch file is left next to it
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("squeezelite.env")]);
    }

    #[test]
    fn a_missing_file_is_created_with_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mestro.env");
        assert_eq!(get_key_in_file(&file, "TZ"), None);
        set_key_in_file(&file, "TZ", "Europe/Berlin").unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "TZ=Europe/Berlin\n"
        );
    }

    #[test]
    fn a_file_that_is_not_utf8_is_refused_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mestro.env");
        let bytes = b"# caf\xe9\nTZ=UTC\n";
        std::fs::write(&file, bytes).unwrap();
        assert!(set_key_in_file(&file, "TZ", "Europe/Berlin").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }

    #[test]
    fn an_unchanged_value_does_not_rewrite_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("squeezelite.env");
        std::fs::write(&file, SQUEEZELITE).unwrap();
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        set_key_in_file(&file, "OUTPUT", "hw:CARD=DAC").unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
    }
}
