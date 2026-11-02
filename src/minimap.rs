//! The editor's minimap, as VS Code's default (`editor.minimap.enabled`, `renderCharacters`, `showSlider: "mouseover"`):
//! every character a 1×2 px block in its syntax colour, the visible part under a slider. Pure geometry; drawn in `tabs`.

/// Width of the minimap column, and how many columns of text it shows.
pub const WIDTH: f32 = 100.;
pub const COLS: usize = 100;
/// A character is a 1 px wide, 2 px tall block (VS Code's `minimap.scale` 1).
pub const CHAR_W: f32 = 1.;
pub const LINE_H: f32 = 2.;
const TAB: usize = 4;

/// Consecutive non-blank characters of one colour on a line, in columns (tabs expanded).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run<C> {
    pub col: u16,
    pub len: u16,
    pub color: C,
}

/// `line`'s runs up to `COLS`; `color_at` gives the colour of the character at a byte offset.
pub fn runs<C: Copy + PartialEq>(line: &str, color_at: impl Fn(usize) -> C) -> Vec<Run<C>> {
    let mut out: Vec<Run<C>> = Vec::new();
    let mut col = 0;
    for (i, ch) in line.char_indices() {
        if col >= COLS {
            break;
        }
        let w = if ch == '\t' { TAB - col % TAB } else { 1 };
        if !ch.is_whitespace() {
            let color = color_at(i);
            match out.last_mut() {
                Some(r) if r.color == color && r.col as usize + r.len as usize == col => r.len += 1,
                _ => out.push(Run { col: col as u16, len: 1, color }),
            }
        }
        col += w;
    }
    out
}

/// `line` as drawn in miniature: tabs expanded, cut at `COLS`, with the colour of each stretch as (bytes, colour).
/// The text is one column per `char`, so it lines up with `runs`.
pub fn glyphs<C: Copy + PartialEq>(line: &str, color_at: impl Fn(usize) -> C) -> (String, Vec<(usize, C)>) {
    let (mut text, mut spans) = (String::new(), Vec::<(usize, C)>::new());
    let mut col = 0;
    for (i, ch) in line.char_indices() {
        if col >= COLS {
            break;
        }
        let w = if ch == '\t' { TAB - col % TAB } else { 1 };
        let color = color_at(i);
        let n = if ch == '\t' { w } else { ch.len_utf8() };
        match spans.last_mut() {
            Some((len, c)) if *c == color => *len += n,
            _ => spans.push((n, color)),
        }
        text.push_str(&if ch == '\t' { " ".repeat(w) } else { ch.to_string() });
        col += w;
    }
    (text, spans)
}

/// Where the editor is scrolled, in lines, and how tall the minimap is.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub lines: usize,
    /// First visible line, fractional.
    pub top: f32,
    /// Lines the editor shows at once.
    pub rows: f32,
    /// Minimap height in px.
    pub height: f32,
}

impl View {
    /// How far the minimap itself is scrolled, in px.
    pub fn scroll(&self) -> f32 {
        let extra = (self.lines as f32 * LINE_H - self.height).max(0.);
        match self.max_top() {
            0. => 0.,
            max => extra * (self.top / max).clamp(0., 1.),
        }
    }

    fn max_top(&self) -> f32 {
        (self.lines as f32 - self.rows).max(0.)
    }

    /// The slider's top (relative to the minimap) and height, in px.
    pub fn slider(&self) -> (f32, f32) {
        (self.top * LINE_H - self.scroll(), self.rows * LINE_H)
    }

    /// A click at `y`: the editor's new top line, centring the line under the pointer.
    pub fn top_for_click(&self, y: f32) -> f32 {
        ((y + self.scroll()) / LINE_H - self.rows / 2.).clamp(0., self.max_top())
    }

    /// Dragging the slider to `slider_top`: the editor's new top line.
    pub fn top_for_slider(&self, slider_top: f32) -> f32 {
        // the slider moves `LINE_H` per line, less the minimap's own scroll
        let max = self.max_top();
        let extra = (self.lines as f32 * LINE_H - self.height).max(0.);
        let track = max * LINE_H - extra; // px the slider can travel
        if track <= 0. {
            return self.top.min(max);
        }
        (slider_top * max / track).clamp(0., max)
    }
}

/// The cache key: the minimap of text `version` must be (re)built unless it is built or being built.
pub fn stale(version: u64, built: Option<u64>, building: Option<u64>) -> bool {
    built != Some(version) && building != Some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(line: &str) -> Vec<Run<char>> {
        // colour = 'w' for word characters, 'p' for punctuation
        runs(line, |i| if line.as_bytes()[i].is_ascii_alphanumeric() { 'w' } else { 'p' })
    }

    #[test]
    fn runs_skip_blanks_and_split_on_colour() {
        let r = |col, len, color| Run { col, len, color };
        assert_eq!(class("  fn main() {"), vec![r(2, 2, 'w'), r(5, 4, 'w'), r(9, 2, 'p'), r(12, 1, 'p')]);
        assert_eq!(class(""), vec![]);
    }

    #[test]
    fn tabs_count_four_columns_and_wide_characters_one() {
        let r = |col, len, color| Run { col, len, color };
        assert_eq!(class("\tx"), vec![r(4, 1, 'w')]);
        assert_eq!(class("é=1"), vec![r(0, 2, 'p'), r(2, 1, 'w')], "é is one column, two bytes");
    }

    #[test]
    fn runs_stop_at_the_last_column() {
        let long = "a".repeat(300);
        assert_eq!(class(&long), vec![Run { col: 0, len: COLS as u16, color: 'w' }]);
    }

    #[test]
    fn glyphs_expand_tabs_and_group_colours() {
        let (text, spans) = glyphs("\tab=é", |i| i >= 3);
        assert_eq!(text, "    ab=é");
        assert_eq!(spans, vec![(6, false), (3, true)], "'=' and 'é' (two bytes) share the second colour");
        assert_eq!(glyphs(&"a".repeat(300), |_| 0).0.len(), COLS);
    }

    #[test]
    fn glyphs_and_blocks_cover_the_same_columns() {
        // the same columns are covered either way, so the slider and clicks are unchanged
        let cols: Vec<_> = runs("fn  main()", |_| 'x').iter().flat_map(|r| r.col..r.col + r.len).collect();
        let (text, _) = glyphs("fn  main()", |_| 'x');
        let drawn: Vec<_> = text.char_indices().filter(|(_, c)| *c != ' ').map(|(i, _)| i as u16).collect();
        assert_eq!(cols, drawn, "a glyph is drawn in the column of its block");
    }

    const LONG: View = View { lines: 1000, top: 0., rows: 50., height: 400. };

    #[test]
    fn a_long_file_scrolls_the_minimap_with_the_editor() {
        // 2000 px of minimap in 400: at the end of the file the minimap shows its last 400 px
        assert_eq!((LONG.scroll(), LONG.slider()), (0., (0., 100.)));
        let end = View { top: 950., ..LONG };
        assert_eq!((end.scroll(), end.slider()), (1600., (300., 100.)));
        let mid = View { top: 475., ..LONG };
        assert_eq!((mid.scroll(), mid.slider()), (800., (150., 100.)));
        let past = View { top: 999., ..LONG };
        assert_eq!(past.scroll(), 1600., "scrolled beyond the last line");
    }

    #[test]
    fn a_short_file_keeps_the_minimap_still() {
        let short = View { lines: 100, top: 50., rows: 50., height: 400. };
        assert_eq!((short.scroll(), short.slider()), (0., (100., 100.)));
    }

    #[test]
    fn a_click_centres_the_line_under_the_pointer() {
        assert_eq!(LONG.top_for_click(200.), 75.);
        assert_eq!(LONG.top_for_click(10.), 0.);
        assert_eq!(LONG.top_for_click(399.), 174.5);
        let end = View { top: 950., ..LONG };
        assert_eq!(end.top_for_click(399.), 950., "line 999 can't be centred");
    }

    #[test]
    fn dragging_the_slider_moves_the_editor_with_it() {
        assert_eq!(LONG.top_for_slider(150.), 475.);
        assert_eq!(LONG.top_for_slider(300.), 950.);
        assert_eq!(LONG.top_for_slider(-20.), 0.);
        assert_eq!(LONG.top_for_slider(390.), 950.);
        let short = View { lines: 100, top: 0., rows: 50., height: 400. };
        assert_eq!(short.top_for_slider(100.), 50.);
    }

    #[test]
    fn the_minimap_is_rebuilt_once_per_text_version() {
        assert!(stale(1, None, None), "never built");
        assert!(!stale(1, Some(1), None));
        assert!(!stale(2, Some(1), Some(2)), "already being built");
        assert!(stale(3, Some(1), Some(2)), "edited again while building");
    }
}
