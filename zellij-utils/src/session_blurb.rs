//! The short AI-written description of a session, cached next to the session's other cache
//! files and shown by `zellij ls` and `zellij attach`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthChar;

pub const BLURB_FILE_NAME: &str = "session-blurb.txt";
/// The narrowest room worth spending on a blurb, in terminal columns.
pub const MIN_BLURB_WIDTH: usize = 8;

const DIM: &str = "\u{1b}[2m";
const RESET: &str = "\u{1b}[m";

pub fn blurb_file_path(session_folder: &Path) -> PathBuf {
    session_folder.join(BLURB_FILE_NAME)
}

/// The cached blurb of the session whose cache folder is `session_folder`. Control characters
/// are stripped: the text came from a model and is printed to the user's terminal.
pub fn read_blurb(session_folder: &Path) -> Option<String> {
    let text = fs::read_to_string(blurb_file_path(session_folder)).ok()?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let blurb: String = line.chars().filter(|c| !c.is_control()).collect();
    let blurb = blurb.trim();
    if blurb.is_empty() {
        None
    } else {
        Some(blurb.to_owned())
    }
}

/// Replaces the cached blurb. Readers never observe a partially written file.
pub fn write_blurb(session_folder: &Path, blurb: &str) -> io::Result<()> {
    fs::create_dir_all(session_folder)?;
    let path = blurb_file_path(session_folder);
    let temp_path = path.with_extension("txt.tmp");
    fs::write(&temp_path, blurb)?;
    fs::rename(&temp_path, &path)
}

/// Number of terminal columns `text` occupies, ignoring ANSI escape sequences.
pub fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // a CSI sequence ends at its first byte in '@'..='~'
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            width += c.width().unwrap_or(0);
        }
    }
    width
}

fn fit_to_width(text: &str, width: usize) -> String {
    if visible_width(text) <= width {
        return text.to_owned();
    }
    let mut fitted = String::new();
    let mut used = 0;
    for c in text.chars() {
        let char_width = c.width().unwrap_or(0);
        if used + char_width > width.saturating_sub(1) {
            break;
        }
        fitted.push(c);
        used += char_width;
    }
    fitted.push('…');
    fitted
}

/// Appends ` - <blurb>` (dimmed) to a printed session line. The blurb is shortened with an
/// ellipsis to fit `terminal_width`, and left out when there is no room for it.
pub fn append_blurb(line: &str, blurb: Option<&str>, terminal_width: Option<usize>) -> String {
    let Some(blurb) = blurb else {
        return line.to_owned();
    };
    let prefix = format!("{} - ", line.trim_end());
    let blurb = match terminal_width {
        None => blurb.to_owned(),
        Some(width) => {
            let room = width.saturating_sub(visible_width(&prefix));
            if room < MIN_BLURB_WIDTH {
                return line.to_owned();
            }
            fit_to_width(blurb, room)
        },
    };
    format!("{}{}{}{}", prefix, DIM, blurb, RESET)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const DIM: &str = "\u{1b}[2m";
    const RESET: &str = "\u{1b}[m";

    #[test]
    fn the_blurb_lives_in_the_session_folder() {
        let path = blurb_file_path(Path::new("cache/abc123"));
        assert_eq!(path, Path::new("cache/abc123").join(BLURB_FILE_NAME));
    }

    #[test]
    fn a_written_blurb_can_be_read_back_and_replaced() {
        let folder = tempdir().unwrap();
        write_blurb(folder.path(), "Fixing the build").unwrap();
        assert_eq!(
            read_blurb(folder.path()).as_deref(),
            Some("Fixing the build")
        );
        write_blurb(folder.path(), "Writing docs").unwrap();
        assert_eq!(read_blurb(folder.path()).as_deref(), Some("Writing docs"));
    }

    #[test]
    fn writing_creates_a_missing_folder() {
        let parent = tempdir().unwrap();
        let folder = parent.path().join("new-session");
        write_blurb(&folder, "Hello").unwrap();
        assert_eq!(read_blurb(&folder).as_deref(), Some("Hello"));
    }

    #[test]
    fn a_missing_or_empty_blurb_reads_as_none() {
        let folder = tempdir().unwrap();
        assert_eq!(read_blurb(folder.path()), None);
        assert_eq!(read_blurb(&folder.path().join("no-such-folder")), None);
        std::fs::write(blurb_file_path(folder.path()), "  \n\n").unwrap();
        assert_eq!(read_blurb(folder.path()), None);
    }

    #[test]
    fn only_the_first_line_is_read() {
        let folder = tempdir().unwrap();
        std::fs::write(blurb_file_path(folder.path()), "First\nSecond\n").unwrap();
        assert_eq!(read_blurb(folder.path()).as_deref(), Some("First"));
    }

    #[test]
    fn control_characters_are_stripped_when_reading() {
        let folder = tempdir().unwrap();
        std::fs::write(
            blurb_file_path(folder.path()),
            "Safe\u{1b}[31m red \u{7}bell\u{9d}",
        )
        .unwrap();
        let blurb = read_blurb(folder.path()).unwrap();
        assert!(!blurb.chars().any(|c| c.is_control()), "got {:?}", blurb);
        assert!(blurb.starts_with("Safe"));
    }

    #[test]
    fn visible_width_ignores_ansi_sequences_and_counts_wide_characters() {
        assert_eq!(visible_width("plain"), 5);
        assert_eq!(visible_width("\u{1b}[32;1mgreen\u{1b}[m text"), 10);
        assert_eq!(visible_width("日本"), 4);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn no_blurb_leaves_the_line_exactly_as_it_was() {
        let line = "\u{1b}[32;1mname\u{1b}[m [Created 5m ago] ";
        assert_eq!(append_blurb(line, None, Some(80)), line);
        assert_eq!(append_blurb(line, None, None), line);
    }

    #[test]
    fn the_blurb_is_appended_dimmed_after_the_trimmed_line() {
        let out = append_blurb("name [Created 5m ago] ", Some("Fixing the build"), None);
        assert_eq!(
            out,
            format!("name [Created 5m ago] - {}Fixing the build{}", DIM, RESET)
        );
    }

    #[test]
    fn a_long_blurb_is_shortened_to_the_terminal_width() {
        let line = "name [Created 5m ago] "; // 21 visible columns once trimmed
        let out = append_blurb(
            line,
            Some("Refactoring the whole session manager plugin"),
            Some(40),
        );
        assert!(
            visible_width(&out) <= 40,
            "width was {}",
            visible_width(&out)
        );
        assert!(out.contains('…'));
        assert!(out.contains("Refactoring"));
    }

    #[test]
    fn a_blurb_that_fits_is_not_shortened() {
        let out = append_blurb("name [Created 5m ago] ", Some("Short"), Some(80));
        assert!(!out.contains('…'));
        assert!(out.contains("Short"));
    }

    #[test]
    fn colored_lines_are_measured_by_their_visible_width() {
        let line = "\u{1b}[32;1mname\u{1b}[m [Created \u{1b}[35;1m5m\u{1b}[m ago] ";
        let out = append_blurb(
            line,
            Some("Refactoring the whole session manager plugin"),
            Some(40),
        );
        assert!(
            visible_width(&out) <= 40,
            "width was {}",
            visible_width(&out)
        );
        assert!(out.contains("Refactoring"));
    }

    #[test]
    fn the_blurb_is_left_out_when_there_is_no_room() {
        let line = "name [Created 5m ago] ";
        assert_eq!(append_blurb(line, Some("Anything"), Some(25)), line);
        assert_eq!(append_blurb(line, Some("Anything"), Some(10)), line);
    }
}
