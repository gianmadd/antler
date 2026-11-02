//! Quick Open's modes (files, `>` commands, `:` line) and the Command palette's order. Pure, so it is tested.
use std::ops::Range;

pub enum Mode<'a> {
    Files,
    Commands(&'a str),
    Line(&'a str),
}

/// What the text of the Quick Open field asks for, like VS Code: a leading `>` or `:` switches the mode.
pub fn mode(query: &str) -> Mode<'_> {
    match query.chars().next() {
        Some('>') => Mode::Commands(&query[1..]),
        Some(':') => Mode::Line(&query[1..]),
        _ => Mode::Files,
    }
}

/// The line typed after `:` and the hint to show; the line is `Some` only between 1 and `total`.
/// `current` is the caret's line. (shortcut: no `line:column` or negative lines, which VS Code takes.)
pub fn line_hint(rest: &str, current: usize, total: usize) -> (String, Option<usize>) {
    match rest.trim().parse::<usize>().ok().filter(|n| (1..=total).contains(n)) {
        Some(n) => (format!("Press 'Enter' to go to line {n}."), Some(n)),
        None => (format!("Current line: {current}. Type a line number between 1 and {total} to go to."), None),
    }
}

/// The ids and titles that match `query` (letters in order, any case), best first: recently used commands
/// (`recent` ids, newest first) lead, then the better matches; the ranges are the matched bytes of the title.
pub fn rank(items: &[(&str, &str)], query: &str, recent: &[String]) -> Vec<(usize, Vec<Range<usize>>)> {
    let letters: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    let mut found: Vec<(usize, i32, Vec<Range<usize>>)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, (_, title))| crate::goto::fuzzy(&letters, title).map(|(score, ranges)| (i, score, ranges)))
        .collect();
    let pos = |i: usize| recent.iter().position(|r| r == items[i].0).unwrap_or(usize::MAX);
    // with a query, the match decides first and recency breaks ties; without one, recency alone
    found.sort_by_key(|(i, score, _)| (std::cmp::Reverse(if letters.is_empty() { 0 } else { *score }), pos(*i)));
    found.into_iter().map(|(i, _, ranges)| (i, ranges)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_symbol_picks_the_mode() {
        assert!(matches!(mode("src"), Mode::Files));
        assert!(matches!(mode(">save"), Mode::Commands("save")));
        assert!(matches!(mode(":42"), Mode::Line("42")));
        assert!(matches!(mode(""), Mode::Files));
    }

    #[test]
    fn a_line_must_be_inside_the_file() {
        assert_eq!(line_hint(" 42 ", 1, 100).1, Some(42));
        for bad in ["", "0", "101", "x", "-3"] {
            let (hint, line) = line_hint(bad, 7, 100);
            assert_eq!(line, None, "{bad}");
            assert_eq!(hint, "Current line: 7. Type a line number between 1 and 100 to go to.");
        }
    }

    #[test]
    fn recent_commands_lead_and_a_query_filters_and_ranks() {
        let items = [("a", "Save"), ("b", "Save All"), ("c", "Close Folder"), ("d", "Toggle Sidebar")];
        let order = |q: &str, recent: &[&str]| -> Vec<usize> {
            rank(&items, q, &recent.iter().map(|s| s.to_string()).collect::<Vec<_>>()).into_iter().map(|(i, _)| i).collect()
        };
        assert_eq!(order("", &[]), [0, 1, 2, 3]);
        assert_eq!(order("", &["d", "b"]), [3, 1, 0, 2]);
        assert_eq!(order("sv", &[]), [0, 1]);
        assert_eq!(order("save", &["b"]), [1, 0], "equal matches: the recent one first");
        assert_eq!(order("zzz", &[]), Vec::<usize>::new());
    }
}
