//! VS Code colour themes (the JSON of an extension's `contributes.themes`) as Antler themes: `colors` onto the palette keys
//! (a missing key keeps Antler Dark's or Light's colour, by the theme's kind), TextMate `tokenColors` onto tree-sitter highlight names.
//! The scope mapping is approximate: a highlight name takes the most specific rule matching one of its candidate scopes.
use std::path::Path;

use serde_json::{Value, json};

use crate::themes::{self, Palette, ThemeDef, *};

/// JSON with `//` and `/* */` comments and trailing commas (what VS Code theme files are) as plain JSON.
pub fn strip_jsonc(text: &str) -> String {
    let (mut out, mut chars) = (String::with_capacity(text.len()), text.chars().peekable());
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                out.push(ch);
                while let Some(s) = chars.next() {
                    out.push(s);
                    if s == '\\' {
                        out.extend(chars.next());
                    } else if s == '"' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                while chars.next_if(|c| *c != '\n').is_some() {}
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '*' && chars.next_if_eq(&'/').is_some() {
                        break;
                    }
                }
                out.push(' ');
            }
            _ => out.push(ch),
        }
    }
    // trailing commas: a comma followed (after whitespace) by } or ]
    let bytes: Vec<char> = out.chars().collect();
    let (mut clean, mut in_str, mut i) = (String::with_capacity(bytes.len()), false, 0);
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            clean.push(c);
            if c == '\\' {
                i += 1;
                clean.extend(bytes.get(i));
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            clean.push(c);
        } else if c == ',' && bytes[i + 1..].iter().find(|c| !c.is_whitespace()).is_some_and(|c| matches!(c, '}' | ']')) {
        } else {
            clean.push(c);
        }
        i += 1;
    }
    clean
}

/// A theme file with its `include` chain merged (the including file wins; `tokenColors` are appended after the included ones).
/// An included file must lie inside `root` (the extension's folder).
pub fn read(file: &Path, root: &Path) -> Result<Value, String> {
    fn go(file: &Path, root: &Path, depth: u8) -> Result<Value, String> {
        let canon = file.canonicalize().map_err(|e| format!("{}: {e}", file.display()))?;
        if !canon.starts_with(root) {
            return Err(format!("{} is outside the extension", file.display()));
        }
        let text = std::fs::read_to_string(&canon).map_err(|e| e.to_string())?;
        let theme: Value = serde_json::from_str(&strip_jsonc(&text)).map_err(|e| format!("{}: {e}", file.display()))?;
        let Some(inc) = theme.get("include").and_then(Value::as_str).filter(|_| depth < 8) else { return Ok(theme) };
        let mut base = go(&canon.parent().unwrap_or(root).join(inc), root, depth + 1)?;
        if let (Some(b), Some(t)) = (base["colors"].as_object_mut(), theme["colors"].as_object()) {
            b.extend(t.clone());
        } else if theme["colors"].is_object() {
            base["colors"] = theme["colors"].clone();
        }
        let tokens: Vec<Value> = [&base["tokenColors"], &theme["tokenColors"]].iter().filter_map(|v| v.as_array()).flatten().cloned().collect();
        base["tokenColors"] = tokens.into();
        if let Some(t) = theme.get("type") {
            base["type"] = t.clone();
        }
        Ok(base)
    }
    go(file, &root.canonicalize().map_err(|e| e.to_string())?, 0)
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` as (0xRRGGBB, alpha).
fn parse_color(s: &str) -> Option<(u32, f32)> {
    let hex = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    let nib = |shift: u32| (v >> shift & 0xf) * 0x11;
    match hex.len() {
        6 => Some((v, 1.)),
        8 => Some((v >> 8, (v & 0xff) as f32 / 255.)),
        3 => Some((nib(8) << 16 | nib(4) << 8 | nib(0), 1.)),
        4 => Some((nib(12) << 16 | nib(8) << 8 | nib(4), nib(0) as f32 / 255.)),
        _ => None,
    }
}

/// `a` over `b` at `t` (0 is `b`), per channel.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| ((a >> s & 0xff) as f32 * t + (b >> s & 0xff) as f32 * (1. - t)).round() as u32;
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

fn luminance(c: u32) -> f32 {
    (0.2126 * (c >> 16 & 0xff) as f32 + 0.7152 * (c >> 8 & 0xff) as f32 + 0.0722 * (c & 0xff) as f32) / 255.
}

/// The palette of a theme's `colors`; a key the theme lacks keeps `base`'s colour.
pub fn palette(colors: &serde_json::Map<String, Value>, base: &Palette) -> Palette {
    let mut p = *base;
    let main = colors.get("editor.background").and_then(Value::as_str).and_then(parse_color).map_or(base[MAIN], |(c, a)| mix(c, base[MAIN], a));
    // a colour from the first of `keys` the theme sets; a translucent one is laid over the editor background
    let get = |keys: &[&str]| keys.iter().find_map(|k| colors.get(*k)?.as_str().and_then(parse_color)).map(|(c, a)| mix(c, main, a));
    let set = |p: &mut Palette, key: usize, keys: &[&str]| {
        if let Some(c) = get(keys) {
            p[key] = c;
        }
    };
    p[MAIN] = main;
    set(&mut p, ACT, &["activityBar.background", "statusBar.background", "titleBar.activeBackground"]);
    set(&mut p, SIDE, &["sideBar.background", "activityBar.background"]);
    set(&mut p, RAISE, &["input.background", "dropdown.background", "badge.background"]);
    set(&mut p, HOVER, &["list.hoverBackground"]);
    set(&mut p, SEL, &["list.activeSelectionBackground", "list.inactiveSelectionBackground"]);
    set(&mut p, LINE, &["panel.border", "sideBar.border", "editorGroup.border", "contrastBorder"]);
    set(&mut p, LINE_SOFT, &["editorGroup.border", "sideBar.border"]);
    if get(&["editorGroup.border", "sideBar.border"]).is_none() && get(&["panel.border", "contrastBorder"]).is_some() {
        p[LINE_SOFT] = mix(p[LINE], p[MAIN], 0.5);
    }
    set(&mut p, POPOVER, &["editorWidget.background", "menu.background", "dropdown.background", "sideBar.background"]);
    set(&mut p, FG, &["foreground", "editor.foreground"]);
    set(&mut p, FG_DIM, &["descriptionForeground", "sideBarTitle.foreground"]);
    if get(&["descriptionForeground", "sideBarTitle.foreground"]).is_none() && get(&["foreground", "editor.foreground"]).is_some() {
        p[FG_DIM] = mix(p[FG], p[MAIN], 0.65);
    }
    set(&mut p, FG_FAINT, &["disabledForeground", "editorLineNumber.foreground"]);
    if get(&["disabledForeground", "editorLineNumber.foreground"]).is_none() && get(&["foreground", "editor.foreground"]).is_some() {
        p[FG_FAINT] = mix(p[FG], p[MAIN], 0.4);
    }
    set(&mut p, ACCENT, &["button.background", "focusBorder", "textLink.foreground"]);
    p[ON_ACCENT] = get(&["button.foreground"]).unwrap_or(if luminance(p[ACCENT]) > 0.5 { 0x181818 } else { 0xffffff });
    set(&mut p, DANGER, &["errorForeground", "editorError.foreground"]);
    set(&mut p, GIT_M, &["gitDecoration.modifiedResourceForeground"]);
    set(&mut p, GIT_A, &["gitDecoration.addedResourceForeground", "gitDecoration.untrackedResourceForeground"]);
    set(&mut p, GIT_D, &["gitDecoration.deletedResourceForeground"]);
    set(&mut p, GIT_R, &["gitDecoration.renamedResourceForeground", "gitDecoration.untrackedResourceForeground"]);
    set(&mut p, GIT_C, &["gitDecoration.conflictingResourceForeground"]);
    set(&mut p, MODIFIED, &["editorGutter.modifiedBackground"]);
    set(&mut p, LANE1, &["terminal.ansiMagenta"]);
    set(&mut p, LANE2, &["terminal.ansiGreen"]);
    set(&mut p, LANE3, &["terminal.ansiBlue"]);
    set(&mut p, LANE4, &["terminal.ansiRed"]);
    set(&mut p, LANE5, &["terminal.ansiCyan"]);
    p[CHECK_A] = mix(p[FG], p[MAIN], 0.06);
    p[CHECK_B] = mix(p[FG], p[MAIN], 0.14);
    p
}

/// gpui-component's highlight names with the TextMate scopes that give them a style, most specific first.
const SCOPES: &[(&str, &[&str])] = &[
    ("attribute", &["entity.other.attribute-name"]),
    ("boolean", &["constant.language.boolean", "constant.language"]),
    ("comment", &["comment"]),
    ("comment.doc", &["comment.block.documentation", "comment.documentation", "comment"]),
    ("constant", &["variable.other.constant", "support.constant", "constant"]),
    ("constructor", &["entity.name.function.constructor", "entity.name.type.class", "entity.name.type", "support.class"]),
    ("emphasis", &["markup.italic"]),
    ("emphasis.strong", &["markup.bold"]),
    ("enum", &["entity.name.type.enum", "entity.name.type", "support.type"]),
    ("function", &["entity.name.function", "support.function", "meta.function-call"]),
    ("keyword", &["keyword.control", "keyword", "storage.type", "storage"]),
    ("label", &["entity.name.label"]),
    ("link_text", &["string.other.link.title", "markup.underline.link", "string.other.link"]),
    ("link_uri", &["markup.underline.link", "string.other.link"]),
    ("number", &["constant.numeric"]),
    ("operator", &["keyword.operator"]),
    ("preproc", &["meta.preprocessor", "keyword.control.directive", "punctuation.definition.directive"]),
    ("property", &["variable.other.property", "support.type.property-name", "meta.object-literal.key", "variable.object.property", "variable.other.object.property"]),
    ("punctuation", &["punctuation"]),
    ("punctuation.bracket", &["punctuation.definition.block", "punctuation"]),
    ("punctuation.delimiter", &["punctuation.separator", "punctuation.terminator", "punctuation"]),
    ("punctuation.special", &["punctuation.definition.template-expression", "punctuation"]),
    ("string", &["string"]),
    ("string.escape", &["constant.character.escape"]),
    ("string.regex", &["string.regexp"]),
    ("string.special", &["string.other", "constant.other.symbol"]),
    ("string.special.symbol", &["constant.other.symbol"]),
    ("tag", &["entity.name.tag"]),
    ("tag.doctype", &["meta.tag.sgml.doctype", "entity.name.tag"]),
    ("text.code.span", &["markup.inline.raw", "markup.raw"]),
    ("text.literal", &["markup.inline.raw", "markup.raw"]),
    ("title", &["markup.heading", "entity.name.section"]),
    ("type", &["entity.name.type", "support.type", "support.class", "storage.type"]),
    ("variable", &["variable"]),
    ("variable.special", &["variable.language"]),
    ("variant", &["variable.other.enummember", "entity.name.type.enum-member"]),
];

struct Rule {
    scope: String,
    foreground: Option<String>,
    font_style: Option<String>,
}

fn rules(token_colors: &Value) -> Vec<Rule> {
    let mut out = Vec::new();
    for t in token_colors.as_array().into_iter().flatten() {
        let scopes: Vec<String> = match &t["scope"] {
            Value::String(s) => s.split(',').map(|s| s.trim().to_string()).collect(),
            Value::Array(a) => a.iter().filter_map(Value::as_str).flat_map(|s| s.split(',')).map(|s| s.trim().to_string()).collect(),
            _ => continue,
        };
        let s = &t["settings"];
        for scope in scopes.into_iter().filter(|s| !s.is_empty()) {
            out.push(Rule { scope, foreground: s["foreground"].as_str().map(str::to_owned), font_style: s["fontStyle"].as_str().map(str::to_owned) });
        }
    }
    out
}

/// A rule's scope is a selector: `a.b` matches `a.b` and `a.b.c`; a space-separated one (a descendant selector) is taken as its last word.
fn covers(rule: &str, scope: &str) -> bool {
    let rule = rule.rsplit(' ').next().unwrap_or(rule);
    scope == rule || scope.strip_prefix(rule).is_some_and(|rest| rest.starts_with('.'))
}

/// The syntax colours as the JSON `SyntaxColors` reads, or `None` when no rule maps to a highlight name.
pub fn syntax(token_colors: &Value) -> Option<String> {
    let rules = rules(token_colors);
    let mut out = serde_json::Map::new();
    for (name, candidates) in SCOPES {
        // the first candidate some rule covers; among those rules the longest scope, the later one on a tie
        let Some(rule) = candidates.iter().find_map(|c| rules.iter().filter(|r| covers(&r.scope, c)).max_by_key(|r| r.scope.len())) else { continue };
        let rule = candidates.iter().find_map(|c| rules.iter().rfind(|r| covers(&r.scope, c) && r.scope.len() == rule.scope.len())).unwrap_or(rule);
        let mut style = serde_json::Map::new();
        if let Some(color) = rule.foreground.as_deref().and_then(parse_color) {
            style.insert("color".into(), json!(format!("#{:06X}", color.0)));
        }
        let fs = rule.font_style.as_deref().unwrap_or("");
        if fs.contains("italic") {
            style.insert("font_style".into(), json!("italic"));
        }
        if fs.contains("bold") {
            style.insert("font_weight".into(), json!(700));
        }
        if !style.is_empty() {
            out.insert((*name).into(), style.into());
        }
    }
    (!out.is_empty()).then(|| Value::Object(out).to_string())
}

/// The theme `label` from a merged theme file (see [`read`]); `ui_theme` is the manifest's ("vs", "vs-dark", "hc-black", "hc-light").
/// Name and syntax are leaked: a theme lives as long as the app, and there are few.
pub fn import(label: &str, ui_theme: &str, theme: &Value) -> &'static ThemeDef {
    let dark = !matches!(ui_theme, "vs" | "hc-light") && theme["type"].as_str().is_none_or(|t| !matches!(t, "light" | "hcLight"));
    let base = if dark { &themes::THEMES[0] } else { &themes::THEMES[1] };
    let empty = serde_json::Map::new();
    let palette = palette(theme["colors"].as_object().unwrap_or(&empty), &base.palette);
    let syntax = syntax(&theme["tokenColors"]).map(|s| &*Box::leak(s.into_boxed_str()));
    // no token colours (or none that map): keep the base's syntax colours
    Box::leak(Box::new(ThemeDef { name: Box::leak(label.to_string().into_boxed_str()), dark, palette, syntax: syntax.or(base.syntax) }))
}

/// A theme file of an installed extension (`root` is the extension's folder).
pub fn load(label: &str, ui_theme: &str, file: &Path, root: &Path) -> Result<&'static ThemeDef, String> {
    Ok(import(label, ui_theme, &read(file, root)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas_go() {
        let text = "{\n // c\n \"a\": \"x // not a comment\", /* b */ \"b\": [1, 2,],\n \"c\": {\"d\": 1,},\n}";
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(v, json!({"a": "x // not a comment", "b": [1, 2], "c": {"d": 1}}));
        assert_eq!(strip_jsonc(r#"{"a":"q\"//x"}"#), r#"{"a":"q\"//x"}"#);
    }

    #[test]
    fn colours_with_alpha() {
        assert_eq!(parse_color("#ff0000"), Some((0xff0000, 1.)));
        assert_eq!(parse_color("#f00"), Some((0xff0000, 1.)));
        assert_eq!(parse_color("#00000080").map(|c| c.0), Some(0));
        assert!((parse_color("#00000080").unwrap().1 - 0.502).abs() < 0.01);
        assert_eq!(parse_color("red"), None);
        assert_eq!(mix(0xffffff, 0x000000, 0.5), 0x808080);
    }

    fn colors(j: Value) -> serde_json::Map<String, Value> {
        j.as_object().unwrap().clone()
    }

    #[test]
    fn workbench_colours_map_onto_the_palette_and_the_rest_falls_back() {
        let base = themes::find("Antler Dark").palette;
        let p = palette(&colors(json!({"editor.background": "#101010", "sideBar.background": "#202020", "list.activeSelectionBackground": "#ffffff20", "button.background": "#007acc"})), &base);
        assert_eq!((p[MAIN], p[SIDE], p[ACCENT]), (0x101010, 0x202020, 0x007acc));
        assert_eq!(p[SEL], mix(0xffffff, 0x101010, 32. / 255.), "a translucent colour is laid over the editor");
        assert_eq!(p[ON_ACCENT], 0xffffff, "white on a dark accent");
        assert_eq!((p[GIT_M], p[LANE1]), (base[GIT_M], base[LANE1]), "missing keys keep the base");
        let q = palette(&colors(json!({"editor.background": "#ffffff", "button.background": "#ffd000"})), &themes::find("Antler Light").palette);
        assert_eq!(q[ON_ACCENT], 0x181818, "dark text on a light accent");
    }

    #[test]
    fn scopes_map_to_highlight_names() {
        let tokens = json!([
            {"scope": "comment", "settings": {"foreground": "#6A9955", "fontStyle": "italic"}},
            {"scope": ["keyword", "storage.type"], "settings": {"foreground": "#569CD6"}},
            {"scope": "keyword.control", "settings": {"foreground": "#C586C0"}},
            {"scope": "entity.name.function, support.function", "settings": {"foreground": "#DCDCAA"}},
            {"scope": "string", "settings": {"foreground": "#111111"}},
            {"scope": "string", "settings": {"foreground": "#CE9178"}},
            {"scope": "markup.bold", "settings": {"fontStyle": "bold"}},
        ]);
        let v: Value = serde_json::from_str(&syntax(&tokens).unwrap()).unwrap();
        assert_eq!(v["comment"], json!({"color": "#6A9955", "font_style": "italic"}));
        assert_eq!(v["comment.doc"]["color"], "#6A9955", "falls back to the comment rule");
        assert_eq!(v["keyword"]["color"], "#C586C0", "the more specific rule wins");
        assert_eq!(v["function"]["color"], "#DCDCAA");
        assert_eq!(v["string"]["color"], "#CE9178", "the later rule wins a tie");
        assert_eq!(v["emphasis.strong"], json!({"font_weight": 700}));
        assert!(v.get("number").is_none(), "no rule, no entry");
        serde_json::from_value::<gpui_kit::component::highlighter::SyntaxColors>(v).unwrap();
        assert!(syntax(&json!([])).is_none());
    }

    #[test]
    fn include_merges_a_base_file_of_the_same_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("themes");
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("base.json"), r##"{"colors": {"editor.background": "#111111", "foreground": "#aaaaaa"}, "tokenColors": [{"scope": "string", "settings": {"foreground": "#00ff00"}}]}"##).unwrap();
        std::fs::write(d.join("t.json"), r##"{ // child
            "include": "./base.json", "type": "light",
            "colors": {"editor.background": "#eeeeee"},
            "tokenColors": [{"scope": "comment", "settings": {"foreground": "#888888"}}],
        }"##).unwrap();
        let v = read(&d.join("t.json"), tmp.path()).unwrap();
        assert_eq!((v["colors"]["editor.background"].as_str(), v["colors"]["foreground"].as_str()), (Some("#eeeeee"), Some("#aaaaaa")));
        assert_eq!(v["tokenColors"].as_array().unwrap().len(), 2);
        let t = import("Child", "vs-dark", &v);
        assert!(!t.dark, "the file's own type says light");
        assert_eq!(t.palette[MAIN], 0xeeeeee);
        // an include outside the extension is refused
        std::fs::write(tmp.path().join("outside.json"), "{}").unwrap();
        std::fs::create_dir(tmp.path().join("ext")).unwrap();
        std::fs::write(tmp.path().join("ext/t.json"), r#"{"include": "../outside.json"}"#).unwrap();
        assert!(read(&tmp.path().join("ext/t.json"), &tmp.path().join("ext")).unwrap_err().contains("outside"));
    }
}
