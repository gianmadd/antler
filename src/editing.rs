//! VS Code's line commands (comment, move, copy, delete, insert line) and bracket matching, as pure text edits.
//! Offsets are UTF-8 bytes, as the editor's `selected_range` gives them.
use std::ops::Range;

/// One edit: replace `range` of the old text with `text`, then select `select` (offsets in the new text).
#[derive(Debug, PartialEq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub select: Range<usize>,
}

/// The language's line comment and block comment tokens, keyed like `FileTab::language` (extension or name).
fn comments(lang: &str) -> (Option<&'static str>, Option<(&'static str, &'static str)>) {
    const C: Option<(&str, &str)> = Some(("/*", "*/"));
    const XML: Option<(&str, &str)> = Some(("<!--", "-->"));
    match lang {
        "rs" | "rust" | "c" | "cpp" | "cs" | "csharp" | "java" | "js" | "javascript" | "ts" | "typescript" | "tsx" | "go"
        | "swift" | "kt" | "kts" | "kotlin" | "scala" | "php" | "dart" | "proto" | "json" | "jsonc" | "scss" | "less"
        | "groovy" | "gradle" => (Some("//"), C),
        "zig" => (Some("//"), None),
        "py" | "python" => (Some("#"), Some(("\"\"\"", "\"\"\""))),
        "rb" | "ruby" | "sh" | "bash" | "fish" | "yml" | "yaml" | "toml" | "makefile" | "make" | "mk" | "cmake" | "r"
        | "pl" | "ex" | "exs" | "elixir" | "graphql" | "gql" | "dockerfile" | "conf" | "gitignore" | "env" | "nix" => (Some("#"), None),
        "lua" => (Some("--"), Some(("--[[", "]]"))),
        "sql" => (Some("--"), C),
        "hs" | "haskell" => (Some("--"), Some(("{-", "-}"))),
        "ini" | "clj" | "lisp" | "el" | "asm" => (Some(";"), None),
        "tex" | "erl" => (Some("%"), None),
        "vim" => (Some("\""), None),
        "css" => (None, C),
        "html" | "xml" | "svg" | "md" | "markdown" | "vue" | "svelte" | "astro" => (None, XML),
        _ => (None, None),
    }
}

fn line_start(t: &str, o: usize) -> usize {
    t[..o].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(t: &str, o: usize) -> usize {
    t[o..].find('\n').map_or(t.len(), |i| o + i)
}

/// The whole lines a selection touches, without the last newline; a selection ending at a line's start leaves
/// that line out, as in VS Code.
fn lines(t: &str, s: &Range<usize>) -> Range<usize> {
    let end = if s.end > s.start && s.end == line_start(t, s.end) { s.end - 1 } else { s.end };
    line_start(t, s.start)..line_end(t, end)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Joins sorted, non-overlapping `edits` of `t` into one `Edit` and carries the selection `s` through them.
fn apply(t: &str, edits: Vec<(Range<usize>, String)>, s: Range<usize>) -> Option<Edit> {
    let range = edits.first()?.0.start..edits.last()?.0.end;
    let mut text = String::new();
    let mut at = range.start;
    for (r, new) in &edits {
        text += &t[at..r.start];
        text += new;
        at = r.end;
    }
    // `stay`: an offset right where text is inserted stays before it (a selection's start keeps the new token)
    let map = |o: usize, stay: bool| {
        let mut d = 0isize;
        for (r, new) in &edits {
            if o < r.start || (o == r.start && stay) {
                break;
            }
            if o < r.end {
                return (r.start as isize + d) as usize + new.len();
            }
            d += new.len() as isize - r.len() as isize;
        }
        (o as isize + d) as usize
    };
    let select = map(s.start, !s.is_empty())..map(s.end, false);
    Some(Edit { range, text, select })
}

/// Cmd+/: comments the selected lines with the language's line comment, or uncomments them when every non-blank
/// line already is. Languages without one (CSS, HTML) wrap the lines in a block comment instead.
pub fn toggle_comment(t: &str, s: Range<usize>, lang: &str) -> Option<Edit> {
    let r = lines(t, &s);
    let token = match comments(lang) {
        (Some(token), _) => token,
        (None, Some(block)) => {
            let text = r.start + indent(&t[r.clone()])..r.end;
            return apply(t, block_edits(t, text, block), s);
        }
        (None, None) => return None,
    };
    let mut rows = Vec::new();
    let mut at = r.start;
    for line in t[r.clone()].split('\n') {
        rows.push((at, line));
        at += line.len() + 1;
    }
    let filled: Vec<_> = rows.iter().copied().filter(|(_, l)| !l.trim().is_empty()).collect();
    let targets = if filled.is_empty() { rows } else { filled };
    let commented = targets.iter().all(|(_, l)| l.trim_start().starts_with(token));
    let edits = if commented {
        targets
            .iter()
            .map(|&(at, l)| {
                let from = at + indent(l);
                let len = token.len() + l.trim_start()[token.len()..].starts_with(' ') as usize;
                (from..from + len, String::new())
            })
            .collect()
    } else {
        let col = targets.iter().map(|(_, l)| indent(l)).min().unwrap_or(0);
        targets.iter().map(|&(at, _)| (at + col..at + col, format!("{token} "))).collect()
    };
    apply(t, edits, s)
}

/// The edits that uncomment `s` (when it is, or sits inside, a block comment) or wrap it in one.
fn block_edits(t: &str, s: Range<usize>, (open, close): (&str, &str)) -> Vec<(Range<usize>, String)> {
    let inner = &t[s.clone()];
    let trimmed = inner.trim();
    let found = if trimmed.len() >= open.len() + close.len() && trimmed.starts_with(open) && trimmed.ends_with(close) {
        let o = s.start + indent(inner);
        Some((o, o + trimmed.len() - close.len()))
    } else {
        // the selection inside a comment: the nearest opener before it, with no closer in between, and the closer after it
        t[..s.start].rfind(open).filter(|&o| !t[o + open.len()..s.start].contains(close)).and_then(|o| {
            let c = s.end + t[s.end..].find(close)?;
            (!t[s.end..c].contains(open)).then_some((o, c))
        })
    };
    match found {
        Some((o, c)) => {
            let after_open = o + open.len() + t[o + open.len()..c].starts_with(' ') as usize;
            let before_close = c - (c > after_open && t[..c].ends_with(' ')) as usize;
            vec![(o..after_open, String::new()), (before_close..c + close.len(), String::new())]
        }
        None => vec![(s.start..s.start, format!("{open} ")), (s.end..s.end, format!(" {close}"))],
    }
}

/// Shift+Alt+A: wraps the selection in the language's block comment, or removes the one it is or sits in.
/// With nothing selected it inserts an empty comment with the cursor inside, as VS Code does.
pub fn toggle_block_comment(t: &str, s: Range<usize>, lang: &str) -> Option<Edit> {
    let tokens = comments(lang).1?;
    let edits = block_edits(t, s.clone(), tokens);
    if s.is_empty() && edits[0].0.is_empty() {
        let text = format!("{}  {}", tokens.0, tokens.1);
        let o = s.start + tokens.0.len() + 1;
        return Some(Edit { range: s, text, select: o..o });
    }
    apply(t, edits, s)
}

/// Alt+Up / Alt+Down: swaps the selected lines with the line above or below; the selection moves with them.
pub fn move_lines(t: &str, s: Range<usize>, up: bool) -> Option<Edit> {
    let r = lines(t, &s);
    let block = &t[r.clone()];
    if up {
        let prev = line_start(t, r.start.checked_sub(1)?);
        let shift = r.start - prev;
        let text = format!("{block}\n{}", &t[prev..r.start - 1]);
        Some(Edit { range: prev..r.end, text, select: s.start - shift..s.end - shift })
    } else {
        let next = line_end(t, (r.end < t.len()).then_some(r.end + 1)?);
        let shift = next - r.end;
        let text = format!("{}\n{block}", &t[r.end + 1..next]);
        Some(Edit { range: r.start..next, text, select: s.start + shift..s.end + shift })
    }
}

/// Shift+Alt+Up / Shift+Alt+Down: duplicates the selected lines above or below; the selection ends on the copy
/// that is on that side.
pub fn copy_lines(t: &str, s: Range<usize>, up: bool) -> Option<Edit> {
    let r = lines(t, &s);
    let block = &t[r.clone()];
    Some(if up {
        Edit { range: r.start..r.start, text: format!("{block}\n"), select: s }
    } else {
        let shift = block.len() + 1;
        Edit { range: r.end..r.end, text: format!("\n{block}"), select: s.start + shift..s.end + shift }
    })
}

/// Shift+Cmd+K: deletes the selected lines; the cursor lands on the line that takes their place (the one above
/// at the end of the text) at the same column. The selection has no direction here, so its end is the cursor.
pub fn delete_lines(t: &str, s: Range<usize>) -> Option<Edit> {
    let r = lines(t, &s);
    let col = t[line_start(t, s.end)..s.end].chars().count();
    let (range, line) = if r.end < t.len() {
        (r.start..r.end + 1, r.end + 1..line_end(t, r.end + 1))
    } else if r.start > 0 {
        (r.start - 1..r.end, line_start(t, r.start - 1)..r.start - 1)
    } else {
        (r.clone(), 0..0)
    };
    let land = line.start.min(range.start);
    let o = land + t[line].chars().take(col).map(char::len_utf8).sum::<usize>();
    Some(Edit { range, text: String::new(), select: o..o })
}

/// Cmd+Enter / Shift+Cmd+Enter: an empty line below the last selected line or above the first, indented like it.
// shortcut: copies the line's indent only, no extra level after `{`; use the editor's smart indent if missed
pub fn insert_line(t: &str, s: Range<usize>, above: bool) -> Option<Edit> {
    let r = lines(t, &s);
    Some(if above {
        let pad = &t[r.start..r.start + indent(&t[r.start..line_end(t, r.start)])];
        let o = r.start + pad.len();
        Edit { range: r.start..r.start, text: format!("{pad}\n"), select: o..o }
    } else {
        let last = line_start(t, r.end);
        let pad = &t[last..last + indent(&t[last..r.end])];
        let o = r.end + 1 + pad.len();
        Edit { range: r.end..r.end, text: format!("\n{pad}"), select: o..o }
    })
}

/// The bracket right after the cursor (else right before it) and its partner, as VS Code highlights them.
// shortcut: brackets inside strings and comments count too, and the search stops 100 KB away; use tree-sitter if it misleads
pub fn matching_bracket(t: &str, cursor: usize) -> Option<[usize; 2]> {
    const LIMIT: usize = 100_000;
    let b = t.as_bytes(); // brackets are ASCII, and no byte of a multibyte character is
    [Some(cursor), cursor.checked_sub(1)].into_iter().flatten().find_map(|i| {
        let (open, close) = match b.get(i)? {
            b'(' | b')' => (b'(', b')'),
            b'[' | b']' => (b'[', b']'),
            b'{' | b'}' => (b'{', b'}'),
            _ => return None,
        };
        let mut depth = 0;
        if b[i] == open {
            for j in i..b.len().min(i + LIMIT) {
                depth += (b[j] == open) as i32 - (b[j] == close) as i32;
                if depth == 0 {
                    return Some([i, j]);
                }
            }
        } else {
            for j in (i.saturating_sub(LIMIT)..=i).rev() {
                depth += (b[j] == close) as i32 - (b[j] == open) as i32;
                if depth == 0 {
                    return Some([j, i]);
                }
            }
        }
        None
    })
}

/// A merge conflict block: each part's byte range; the headers are whole lines, newline included.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// The `<<<<<<<` line.
    pub current_header: Range<usize>,
    pub current: Range<usize>,
    /// diff3 style: from the `|||||||` line to the `=======` line.
    pub base: Option<Range<usize>>,
    pub incoming: Range<usize>,
    /// The `>>>>>>>` line.
    pub incoming_header: Range<usize>,
}

impl Conflict {
    /// The whole block, markers included.
    pub fn range(&self) -> Range<usize> {
        self.current_header.start..self.incoming_header.end
    }
}

/// The conflict blocks of `t`, parsed like VS Code's merge-conflict extension: markers start a line, and a
/// `<<<<<<<` inside an unfinished block (nested) stops the parse there.
pub fn conflicts(t: &str) -> Vec<Conflict> {
    let mut found = Vec::new();
    // the block being read: its start, then where `|||||||` and `=======` start
    let mut open: Option<(usize, Option<usize>, Option<usize>)> = None;
    let mut at = 0;
    while at < t.len() {
        let end = t[at..].find('\n').map_or(t.len(), |i| at + i + 1);
        let line = &t[at..end];
        if line.starts_with("<<<<<<<") && open.is_some() {
            break;
        }
        match &mut open {
            None if line.starts_with("<<<<<<<") => open = Some((at, None, None)),
            Some((_, base @ None, None)) if line.starts_with("|||||||") => *base = Some(at),
            Some((_, _, split @ None)) if line.starts_with("=======") => *split = Some(at),
            Some((start, base, Some(split))) if line.starts_with(">>>>>>>") => {
                let header = *start..line_end(t, *start) + 1;
                found.push(Conflict {
                    current: header.end..base.unwrap_or(*split),
                    base: base.map(|b| b..*split),
                    incoming: line_end(t, *split) + 1..at,
                    current_header: header,
                    incoming_header: at..end,
                });
                open = None;
            }
            _ => {}
        }
        at = end;
    }
    found
}

/// The inline actions above a conflict block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Accept {
    Current,
    Incoming,
    Both,
}

/// Accept Current / Incoming / Both Changes: the block becomes that text, the cursor at its start.
pub fn accept(t: &str, c: &Conflict, which: Accept) -> Edit {
    let (current, incoming) = (&t[c.current.clone()], &t[c.incoming.clone()]);
    let text = match which {
        Accept::Current => current.to_string(),
        Accept::Incoming => incoming.to_string(),
        Accept::Both => format!("{current}{incoming}"),
    };
    let at = c.range().start;
    Edit { range: c.range(), text, select: at..at }
}

/// Byte offset of the start of each line of `t` (always at least `[0]`; a trailing newline adds an empty last line).
pub fn line_starts(t: &str) -> Vec<usize> {
    std::iter::once(0).chain(t.match_indices('\n').map(|(i, _)| i + 1)).collect()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word around byte offset `o` (touching it on either side), like VS Code's word at the cursor.
fn word_at(t: &str, o: usize) -> Option<Range<usize>> {
    let start = t[..o].char_indices().rev().take_while(|&(_, c)| is_word(c)).last().map_or(o, |(i, _)| i);
    let end = o + t[o..].char_indices().take_while(|&(_, c)| is_word(c)).last().map_or(0, |(i, c)| i + c.len_utf8());
    (start < end).then_some(start..end)
}

/// Matches of the text selected by `s`, in order. VS Code's rules: an empty selection searches its word, whole word
/// and case-sensitive; a selection searches its text anywhere, ignoring case (the Find widget's defaults).
/// shortcut: case folding is ASCII only, to keep byte offsets; upgrade if non-ASCII matches matter.
fn matches(t: &str, s: &Range<usize>) -> Vec<Range<usize>> {
    let (needle, whole) = match s.is_empty() {
        true => match word_at(t, s.start) {
            Some(w) => (t[w].to_string(), true),
            None => return vec![],
        },
        false => (t[s.clone()].to_string(), false),
    };
    let (hay, needle) = if whole { (t.to_string(), needle) } else { (t.to_ascii_lowercase(), needle.to_ascii_lowercase()) };
    let ok = |m: &Range<usize>| !whole || (!t[..m.start].ends_with(is_word) && !t[m.end..].starts_with(is_word));
    let mut out: Vec<Range<usize>> = vec![];
    for (i, _) in hay.match_indices(&needle) {
        let m = i..i + needle.len();
        if ok(&m) {
            out.push(m);
        }
    }
    out
}

/// Cmd+D: what to add. An empty selection first becomes its word; otherwise the first match starting at or after
/// `from` (the end of the last added selection), wrapping around to the top. `None`: no word or no match.
pub fn next_occurrence(t: &str, s: Range<usize>, from: usize) -> Option<Range<usize>> {
    if s.is_empty() {
        return word_at(t, s.start);
    }
    let all = matches(t, &s);
    all.iter().find(|m| m.start >= from).or(all.first()).cloned()
}

/// Cmd+Shift+L: every match of the selection (or of the word at an empty one).
pub fn all_occurrences(t: &str, s: Range<usize>) -> Vec<Range<usize>> {
    matches(t, &s)
}

/// Shift+Alt+I: a cursor at the end of every line of a selection; the last line's at the selection's end, unless
/// that is its start. Empty selection: none.
pub fn line_end_cursors(t: &str, s: Range<usize>) -> Vec<usize> {
    let mut out = vec![];
    if s.is_empty() {
        return out;
    }
    let mut o = s.start;
    while line_end(t, o) < s.end {
        out.push(line_end(t, o));
        o = line_end(t, o) + 1;
    }
    if s.end > line_start(t, s.end) {
        out.push(s.end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_of_text() {
        assert_eq!(line_starts(""), [0]);
        assert_eq!(line_starts("a\nbc"), [0, 2]);
        assert_eq!(line_starts("a\nbc\n"), [0, 2, 5]);
    }

    /// Runs `edit` on `text` and returns the new text with the selection drawn as `[` `]` (or `|` when empty).
    fn show(text: &str, edit: Option<Edit>) -> String {
        let Some(e) = edit else { return "unchanged".into() };
        let mut t = format!("{}{}{}", &text[..e.range.start], e.text, &text[e.range.end..]);
        if e.select.is_empty() {
            t.insert(e.select.start, '|');
        } else {
            t.insert(e.select.end, ']');
            t.insert(e.select.start, '[');
        }
        t
    }

    /// `text` with the selection marked by `|` (a cursor) or `[` `]`, split into the plain text and the range.
    fn sel(marked: &str) -> (String, Range<usize>) {
        if let Some(i) = marked.find('|') {
            return (marked.replace('|', ""), i..i);
        }
        let (a, b) = (marked.find('[').unwrap(), marked.find(']').unwrap());
        (marked.replace(['[', ']'], ""), a..b - 1)
    }

    fn run(marked: &str, f: impl FnOnce(&str, Range<usize>) -> Option<Edit>) -> String {
        let (text, range) = sel(marked);
        show(&text, f(&text, range))
    }

    #[test]
    fn line_comment_adds_the_token_at_the_lines_indent_and_keeps_the_cursor_on_its_character() {
        assert_eq!(run("fn a() {\n    let| x;\n}", |t, s| toggle_comment(t, s, "rs")), "fn a() {\n    // let| x;\n}");
    }

    #[test]
    fn line_comment_on_several_lines_uses_their_smallest_indent_and_skips_blank_lines() {
        let rs = |t: &str, s| toggle_comment(t, s, "rs");
        assert_eq!(run("[  a\n\n    b\n]c", rs), "[  // a\n\n  //   b\n]c");
        assert_eq!(run("[  // a\n\n  //   b\n]c", rs), "[  a\n\n    b\n]c");
    }

    #[test]
    fn line_comment_uncomments_only_when_every_line_is_commented() {
        let py = |t: &str, s| toggle_comment(t, s, "py");
        assert_eq!(run("[# a\nb]", py), "[# # a\n# b]");
        assert_eq!(run("#a|", py), "a|");
    }

    #[test]
    fn line_comment_on_an_empty_line_inserts_the_token() {
        assert_eq!(run("a\n|\nb", |t, s| toggle_comment(t, s, "rs")), "a\n// |\nb");
    }

    #[test]
    fn line_comment_in_a_language_without_one_wraps_the_lines_in_a_block_comment() {
        let css = |t: &str, s| toggle_comment(t, s, "css");
        assert_eq!(run("  color: |red;", css), "  /* color: |red; */");
        assert_eq!(run("  /* color: |red; */", css), "  color: |red;");
    }

    #[test]
    fn block_comment_wraps_the_selection_and_toggles_back() {
        let rs = |t: &str, s| toggle_block_comment(t, s, "rs");
        assert_eq!(run("f([a, b]);", rs), "f([/* a, b */]);");
        assert_eq!(run("f([/* a, b */]);", rs), "f([a, b]);");
    }

    #[test]
    fn block_comment_with_no_selection_inserts_an_empty_comment_or_removes_the_one_around_the_cursor() {
        let rs = |t: &str, s| toggle_block_comment(t, s, "rs");
        assert_eq!(run("a|b", rs), "a/* | */b");
        assert_eq!(run("a /* x| y */ b", rs), "a x| y b");
        assert_eq!(run("/* x */ a| /* y */", rs), "/* x */ a/* | */ /* y */");
    }

    #[test]
    fn block_comment_does_nothing_in_a_language_without_one() {
        assert_eq!(run("a|", |t, s| toggle_block_comment(t, s, "yaml")), "unchanged");
    }

    #[test]
    fn move_lines_swaps_the_selected_lines_with_the_one_above_or_below_keeping_the_selection() {
        assert_eq!(run("a\nb|b\nc", |t, s| move_lines(t, s, true)), "b|b\na\nc");
        assert_eq!(run("a\n[b\nc]\nd", |t, s| move_lines(t, s, false)), "a\nd\n[b\nc]");
        assert_eq!(run("a|\nb", |t, s| move_lines(t, s, true)), "unchanged");
        assert_eq!(run("a\nb|", |t, s| move_lines(t, s, false)), "unchanged");
    }

    #[test]
    fn copy_lines_duplicates_the_selected_lines_and_selects_the_copy_on_the_side_it_went() {
        assert_eq!(run("a\nb|b\nc", |t, s| copy_lines(t, s, false)), "a\nbb\nb|b\nc");
        assert_eq!(run("a\nb|b\nc", |t, s| copy_lines(t, s, true)), "a\nb|b\nbb\nc");
        assert_eq!(run("[a\nb]", |t, s| copy_lines(t, s, false)), "a\nb\n[a\nb]");
    }

    #[test]
    fn delete_lines_removes_the_selected_lines_and_keeps_the_column_on_the_next_one() {
        assert_eq!(run("a\nbb|b\ncccc", delete_lines), "a\ncc|cc");
        assert_eq!(run("a\nbb|b\nc", delete_lines), "a\nc|");
        assert_eq!(run("a\n[b\nc]", delete_lines), "a|");
        assert_eq!(run("on|ly", delete_lines), "|");
        assert_eq!(run("é|\nàb", delete_lines), "à|b");
    }

    #[test]
    fn insert_line_opens_an_empty_line_below_or_above_with_the_same_indent() {
        assert_eq!(run("  a|b\nc", |t, s| insert_line(t, s, false)), "  ab\n  |\nc");
        assert_eq!(run("a\n  [b\nc]", |t, s| insert_line(t, s, true)), "a\n  |\n  b\nc");
    }

    #[test]
    fn matching_bracket_pairs_the_bracket_next_to_the_cursor_with_its_partner() {
        let t = "f(a[1], (b)) {}";
        assert_eq!(matching_bracket(t, 1), Some([1, 11])); // before `(`
        assert_eq!(matching_bracket(t, 12), Some([1, 11])); // after `)`
        assert_eq!(matching_bracket(t, 4), Some([3, 5])); // after `[`
        assert_eq!(matching_bracket(t, 14), Some([13, 14])); // between `{` and `}`: the one after wins
        assert_eq!(matching_bracket(t, 0), None);
        assert_eq!(matching_bracket("(a", 0), None); // unclosed
        assert_eq!(matching_bracket("é)", 2), None); // unopened, after a multibyte char
        assert_eq!(matching_bracket("x", 9), None); // stale offset
    }

    #[test]
    fn line_comment_does_nothing_in_an_unknown_language() {
        assert_eq!(run("a|", |t, s| toggle_comment(t, s, "txt")), "unchanged");
    }

    /// The text of each part of every conflict block, for readable asserts.
    fn parts(t: &str) -> Vec<(&str, Option<&str>, &str)> {
        conflicts(t).iter().map(|c| (&t[c.current.clone()], c.base.clone().map(|b| &t[b]), &t[c.incoming.clone()])).collect()
    }

    const TWO: &str = "top\n<<<<<<< HEAD\nmine\n=======\ntheirs\nmore theirs\n>>>>>>> feature\nbottom\n";

    #[test]
    fn conflicts_finds_each_block_with_its_current_and_incoming_text() {
        assert_eq!(parts(TWO), [("mine\n", None, "theirs\nmore theirs\n")]);
        let c = &conflicts(TWO)[0];
        assert_eq!(&TWO[c.range()], "<<<<<<< HEAD\nmine\n=======\ntheirs\nmore theirs\n>>>>>>> feature\n");
        assert_eq!(&TWO[c.current_header.clone()], "<<<<<<< HEAD\n");
        assert_eq!(&TWO[c.incoming_header.clone()], ">>>>>>> feature\n");
        // two blocks, the second without a newline at the end of the file and with an empty side
        let t = "<<<<<<< a\nx\n=======\n>>>>>>> b\nmid\n<<<<<<< a\n=======\ny\n>>>>>>> b";
        assert_eq!(parts(t), [("x\n", None, ""), ("", None, "y\n")]);
        assert_eq!(conflicts(t)[1].range().end, t.len());
        assert!(conflicts("no markers\n=======\n>>>>>>> x\n").is_empty());
        assert!(conflicts("<<<<<<< a\nunfinished\n=======\n").is_empty());
    }

    #[test]
    fn conflicts_reads_the_common_ancestor_of_a_diff3_block() {
        let t = "<<<<<<< HEAD\nmine\n||||||| base\norig\n=======\ntheirs\n>>>>>>> b\n";
        assert_eq!(parts(t), [("mine\n", Some("||||||| base\norig\n"), "theirs\n")]);
    }

    #[test]
    fn conflicts_stops_at_a_nested_block_like_vs_code() {
        let ok = "<<<<<<< a\n1\n=======\n2\n>>>>>>> b\n";
        let nested = "<<<<<<< a\n<<<<<<< c\nx\n=======\ny\n>>>>>>> d\n=======\nz\n>>>>>>> b\n";
        assert_eq!(parts(&format!("{ok}{nested}{ok}")), [("1\n", None, "2\n")]);
    }

    #[test]
    fn accepting_a_side_replaces_the_whole_block_with_it() {
        let c = &conflicts(TWO)[0];
        assert_eq!(show(TWO, Some(accept(TWO, c, Accept::Current))), "top\n|mine\nbottom\n");
        assert_eq!(show(TWO, Some(accept(TWO, c, Accept::Incoming))), "top\n|theirs\nmore theirs\nbottom\n");
        assert_eq!(show(TWO, Some(accept(TWO, c, Accept::Both))), "top\n|mine\ntheirs\nmore theirs\nbottom\n");
        // diff3: the common ancestor goes too
        let t = "<<<<<<< HEAD\nmine\n||||||| base\norig\n=======\ntheirs\n>>>>>>> b";
        assert_eq!(show(t, Some(accept(t, &conflicts(t)[0], Accept::Current))), "|mine\n");
        assert_eq!(show(t, Some(accept(t, &conflicts(t)[0], Accept::Both))), "|mine\ntheirs\n");
    }

    #[test]
    fn next_occurrence_rules() {
        let t = "foo bar Foo foobar foo";
        // empty selection: the word under (or touching) the cursor
        assert_eq!(next_occurrence(t, 1..1, 0), Some(0..3));
        assert_eq!(next_occurrence(t, 3..3, 0), Some(0..3));
        assert_eq!(next_occurrence("a  b", 2..2, 0), None);
        // selection: next match after `from`, ignoring case, inside words too
        assert_eq!(next_occurrence(t, 0..3, 3), Some(8..11));
        assert_eq!(next_occurrence(t, 0..3, 11), Some(12..15));
        assert_eq!(next_occurrence(t, 0..3, 15), Some(19..22));
        // wrap around
        assert_eq!(next_occurrence(t, 19..22, 22), Some(0..3));
        assert_eq!(next_occurrence("é x é", 0..2, 2), Some(5..7));
        assert_eq!(next_occurrence("abc", 0..3, 3), Some(0..3)); // only match: itself
    }

    #[test]
    fn all_occurrences_rules() {
        let t = "foo Foo foobar foo_x foo";
        // empty selection: whole word, case-sensitive
        assert_eq!(all_occurrences(t, 1..1), [0..3, 21..24]);
        assert_eq!(all_occurrences(t, 3..3), [0..3, 21..24]);
        assert_eq!(all_occurrences("x  y", 2..2), Vec::<Range<usize>>::new());
        // selection: any substring, ignoring case
        assert_eq!(all_occurrences(t, 0..3), [0..3, 4..7, 8..11, 15..18, 21..24]);
        assert_eq!(all_occurrences("aaaa", 0..2), [0..2, 2..4]);
    }

    #[test]
    fn line_end_cursors_rules() {
        let t = "ab\ncd\nef";
        assert_eq!(line_end_cursors(t, 1..8), [2, 5, 8]);
        assert_eq!(line_end_cursors(t, 1..6), [2, 5]); // ends at a line start: no cursor there
        assert_eq!(line_end_cursors(t, 0..1), [1]);
        assert!(line_end_cursors(t, 1..1).is_empty());
    }
}
