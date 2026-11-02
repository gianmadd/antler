//! The command table: the one place that says which key runs which command, where. The key bindings, the menu bar,
//! the tooltips and `~/.config/antler/keybindings.json` all read it (the Command palette will too).
//! A command's id is VS Code's when VS Code has the command, so a keybindings.json can be copied over.
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, Replace, Search, SelectAll, Undo};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt as _;
use gpui_kit::*;

use crate::branch_view::CloseBranch;
use crate::explorer::*;
use crate::goto_view::{CloseGoToFile, GoToFile};
use crate::minimap_view::{ToggleMinimap, ToggleRenderCharacters};
use crate::stash_view::CloseStash;
use crate::tabs::*;
use crate::{CheckForUpdates, CloseFolder, CloseTab, Commit, OpenFolder, Quit, Save, SaveAll, ShowExplorer, ShowGit, ShowSearch, ToggleSidebar, set_menus};

/// Where a binding applies: a GPUI key context, and the name `when` uses in keybindings.json.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ctx {
    Global,
    /// Any text field, the code editor included.
    Input,
    Editor,
    Explorer,
    Git,
    Search,
    Commit,
    /// The pickers and the Explorer's name field: for their own Esc only, not offered to `when`.
    GoTo,
    Branch,
    Stash,
    ExplorerName,
}

impl Ctx {
    fn predicate(self) -> Option<&'static str> {
        Some(match self {
            Ctx::Global => return None,
            Ctx::Input => "Input",
            Ctx::Editor => "Input && mode == editor",
            Ctx::Explorer => "Explorer",
            Ctx::Git => "Git",
            Ctx::Search => "Search",
            Ctx::Commit => "CommitBox > Input",
            Ctx::GoTo => "GoToFile > Command",
            Ctx::Branch => "Branch > Command",
            Ctx::Stash => "Stash > Command",
            Ctx::ExplorerName => "ExplorerName > Input",
        })
    }

    /// The contexts keybindings.json's `when` accepts.
    fn from_when(name: &str) -> Option<Ctx> {
        Some(match name {
            "input" => Ctx::Input,
            "editor" => Ctx::Editor,
            "explorer" => Ctx::Explorer,
            "git" => Ctx::Git,
            "search" => Ctx::Search,
            "commit" => Ctx::Commit,
            _ => return None,
        })
    }
}

pub(crate) struct Cmd {
    pub id: &'static str,
    pub title: &'static str,
    /// GPUI's spelling ("cmd-shift-f", chords with a space); "" for a command without a default key.
    pub key: &'static str,
    pub ctx: Ctx,
    action: fn() -> Box<dyn Action>,
}

macro_rules! cmds {
    ($(($id:literal, $title:literal, $key:literal, $ctx:ident, $action:expr)),* $(,)?) => {
        pub(crate) static COMMANDS: &[Cmd] = &[$(Cmd { id: $id, title: $title, key: $key, ctx: Ctx::$ctx, action: || Box::new($action) }),*];
    };
}

// A key bound twice, in the same context, to two commands is a test failure. Several rows with one id are
// several keys (or contexts) of one command. Rows with no key are bound by gpui-component itself or by nothing.
cmds![
    // File
    ("workbench.action.files.newUntitledFile", "New File", "cmd-n", Global, NewUntitledFile),
    ("workbench.action.files.save", "Save", "cmd-s", Global, Save),
    ("workbench.action.files.saveAll", "Save All", "alt-cmd-s", Global, SaveAll),
    ("workbench.action.files.openFolder", "Open Folder…", "cmd-o", Global, OpenFolder),
    ("workbench.action.closeFolder", "Close Folder", "cmd-k f", Global, CloseFolder),
    ("workbench.action.closeActiveEditor", "Close Editor", "cmd-w", Global, CloseTab),
    ("workbench.action.reopenClosedEditor", "Reopen Closed Editor", "shift-cmd-t", Global, ReopenClosedTab),
    ("workbench.action.closeAllEditors", "Close All Editors", "cmd-k cmd-w", Global, CloseAllTabs),
    ("workbench.action.closeOtherEditors", "Close Other Editors", "alt-cmd-t", Global, CloseOtherTabs),
    ("workbench.action.closeUnmodifiedEditors", "Close Saved Editors", "cmd-k u", Global, CloseSavedTabs),
    ("antler.closeEditorsToTheRight", "Close Editors to the Right", "", Global, CloseTabsToTheRight),
    ("workbench.action.keepEditor", "Keep Open", "cmd-k enter", Global, KeepTabOpen),
    ("workbench.action.quit", "Quit Antler", "cmd-q", Global, Quit),
    ("antler.checkForUpdates", "Check for Updates…", "", Global, CheckForUpdates),
    // Edit (the first five belong to the focused text field)
    ("undo", "Undo", "", Input, Undo),
    ("redo", "Redo", "", Input, Redo),
    ("editor.action.clipboardCutAction", "Cut", "", Input, Cut),
    ("editor.action.clipboardCopyAction", "Copy", "", Input, Copy),
    ("editor.action.clipboardPasteAction", "Paste", "", Input, Paste),
    ("actions.find", "Find", "", Input, Search),
    ("editor.action.startFindReplace", "Replace", "cmd-alt-f", Input, Replace),
    ("editor.action.nextMatchFindAction", "Find Next", "cmd-g", Editor, FindNextMatch),
    ("editor.action.previousMatchFindAction", "Find Previous", "shift-cmd-g", Editor, FindPreviousMatch),
    // the editor's own Replace key is cmd-shift-f: project search takes it (bound later, same context, wins)
    ("workbench.view.search", "Find in Files", "shift-cmd-f", Global, ShowSearch),
    ("workbench.view.search", "Find in Files", "shift-cmd-f", Input, ShowSearch),
    ("editor.action.commentLine", "Toggle Line Comment", "cmd-/", Editor, ToggleLineComment),
    ("editor.action.blockComment", "Toggle Block Comment", "alt-shift-a", Editor, ToggleBlockComment),
    ("editor.action.deleteLines", "Delete Line", "cmd-shift-k", Editor, DeleteLine),
    ("editor.action.insertLineAfter", "Insert Line Below", "cmd-enter", Editor, InsertLineBelow),
    ("editor.action.insertLineBefore", "Insert Line Above", "cmd-shift-enter", Editor, InsertLineAbove),
    ("editor.unfold", "Unfold", "cmd-alt-]", Editor, Unfold),
    ("editor.unfoldAll", "Unfold All", "cmd-k cmd-j", Editor, UnfoldAll),
    // Selection
    ("editor.action.selectAll", "Select All", "", Input, SelectAll),
    ("editor.action.addSelectionToNextFindMatch", "Add Next Occurrence", "cmd-d", Editor, AddNextMatch),
    ("editor.action.selectHighlights", "Select All Occurrences", "shift-cmd-l", Editor, SelectAllMatches),
    ("editor.action.selectHighlights", "Select All Occurrences", "cmd-f2", Editor, SelectAllMatches),
    ("editor.action.insertCursorAtEndOfEachLineSelected", "Add Cursors to Line Ends", "shift-alt-i", Editor, CursorsAtLineEnds),
    ("editor.action.copyLinesUpAction", "Copy Line Up", "shift-alt-up", Editor, CopyLineUp),
    ("editor.action.copyLinesDownAction", "Copy Line Down", "shift-alt-down", Editor, CopyLineDown),
    ("editor.action.moveLinesUpAction", "Move Line Up", "alt-up", Editor, MoveLineUp),
    ("editor.action.moveLinesDownAction", "Move Line Down", "alt-down", Editor, MoveLineDown),
    // View
    ("workbench.view.explorer", "Explorer", "shift-cmd-e", Global, ShowExplorer),
    ("workbench.view.scm", "Git", "ctrl-shift-g", Global, ShowGit),
    ("workbench.action.toggleSidebarVisibility", "Toggle Sidebar", "cmd-b", Global, ToggleSidebar),
    ("markdown.showPreview", "Toggle Markdown Preview", "cmd-shift-v", Global, TogglePreview),
    ("editor.action.toggleWordWrap", "Toggle Word Wrap", "alt-z", Editor, ToggleWordWrap),
    ("editor.action.fontZoomIn", "Zoom In", "cmd-=", Editor, ZoomIn),
    ("editor.action.fontZoomIn", "Zoom In", "cmd-+", Editor, ZoomIn), // layouts with a + key of its own (Italian): VS Code binds both
    ("editor.action.fontZoomOut", "Zoom Out", "cmd--", Editor, ZoomOut),
    ("editor.action.fontZoomReset", "Reset Zoom", "cmd-0", Editor, ZoomReset),
    ("antler.toggleMinimap", "Show Minimap", "", Global, ToggleMinimap),
    ("antler.toggleMinimapCharacters", "Render Characters", "", Global, ToggleRenderCharacters),
    // Go
    ("workbench.action.quickOpen", "Go to File…", "cmd-p", Global, GoToFile),
    ("workbench.action.nextEditor", "Next Editor", "alt-cmd-right", Global, NextTab),
    ("workbench.action.previousEditor", "Previous Editor", "alt-cmd-left", Global, PreviousTab),
    ("workbench.action.openNextRecentlyUsedEditor", "Next Used Editor", "ctrl-tab", Global, NextRecentTab),
    ("workbench.action.openPreviousRecentlyUsedEditor", "Previous Used Editor", "ctrl-shift-tab", Global, PreviousRecentTab),
    ("workbench.action.editor.nextChange", "Next Change", "alt-f5", Global, NextChange),
    ("workbench.action.editor.previousChange", "Previous Change", "shift-alt-f5", Global, PreviousChange),
    // the active file (VS Code's cmd-k chords, which work anywhere), and the selected row of the Explorer or Git view
    ("workbench.action.files.copyPathOfActiveFile", "Copy Path of Active File", "cmd-k alt-cmd-c", Global, CopyTabPath),
    ("workbench.action.files.copyRelativePathOfActiveFile", "Copy Relative Path of Active File", "cmd-k shift-alt-cmd-c", Global, CopyRelativeTabPath),
    ("workbench.action.files.revealActiveFileInWindows", "Reveal Active File in Finder", "cmd-k r", Global, RevealTabInFinder),
    ("antler.revealActiveFileInExplorer", "Reveal Active File in Explorer", "", Global, RevealTabInExplorer),
    ("copyFilePath", "Copy Path", "alt-cmd-c", Explorer, CopyEntryPath),
    ("copyFilePath", "Copy Path", "alt-cmd-c", Git, CopyEntryPath),
    ("copyRelativeFilePath", "Copy Relative Path", "shift-alt-cmd-c", Explorer, CopyEntryRelativePath),
    ("copyRelativeFilePath", "Copy Relative Path", "shift-alt-cmd-c", Git, CopyEntryRelativePath),
    // Explorer
    ("renameFile", "Rename", "enter", Explorer, RenameEntry),
    ("moveFileToTrash", "Delete", "cmd-backspace", Explorer, DeleteEntry),
    ("antler.explorer.undo", "Undo File Operation", "cmd-z", Explorer, UndoFileOp),
    ("antler.explorer.redo", "Redo File Operation", "shift-cmd-z", Explorer, RedoFileOp),
    ("antler.explorer.clearFilter", "Clear Find", "escape", Explorer, ClearFilter),
    ("antler.explorer.findNext", "Next Match", "down", Explorer, FindNext),
    ("antler.explorer.findPrevious", "Previous Match", "up", Explorer, FindPrev),
    ("filesExplorer.copy", "Copy", "cmd-c", Explorer, CopyEntries),
    ("filesExplorer.cut", "Cut", "cmd-x", Explorer, CutEntries),
    ("filesExplorer.paste", "Paste", "cmd-v", Explorer, PasteEntries),
    ("antler.explorer.cancelName", "Cancel Name", "escape", ExplorerName, CancelName),
    // Git
    ("git.commit", "Commit", "cmd-enter", Commit, Commit),
    // the pickers: Esc closes right away (the palette's own Esc first clears the query)
    ("antler.closeGoToFile", "Close Go to File", "escape", GoTo, CloseGoToFile),
    ("antler.closeBranchPicker", "Close Branch Picker", "escape", Branch, CloseBranch),
    ("antler.closeStashPicker", "Close Stash Picker", "escape", Stash, CloseStash),
];

/// One key of one command in one context, as the user's keybindings.json leaves it.
#[derive(Clone)]
pub(crate) struct Binding {
    cmd: &'static Cmd,
    /// GPUI's spelling, canonical: modifiers in GPUI's order.
    key: String,
    ctx: Ctx,
}

/// The bindings in force: what the tooltips read.
struct Active(Vec<Binding>);
impl Global for Active {}

fn parse(chord: &str) -> Option<Keystroke> {
    Keystroke::parse_with_os(chord, OperatingSystem::Mac).ok()
}

/// `chord` in GPUI's canonical spelling, if it is a key.
fn canon(chord: &str) -> Option<String> {
    parse(chord).map(|k| k.unparse(OperatingSystem::Mac))
}

/// VS Code's "cmd+shift+f ctrl+k" as GPUI's "cmd-shift-f ctrl-k".
fn from_vscode(key: &str) -> Option<String> {
    let chords: Vec<String> = key
        .to_lowercase()
        .split_whitespace()
        .map(|chord| {
            // a key that is a "+" itself ends the chord with "++"
            let (mods, k) = chord.strip_suffix("++").map(|m| (m, "+")).unwrap_or_else(|| chord.rsplit_once('+').unwrap_or(("", chord)));
            let mods = mods.split('+').filter(|m| !m.is_empty()).map(|m| match m {
                "meta" | "win" | "super" => "cmd",
                "option" => "alt",
                m => m,
            });
            let k = match k {
                "esc" => "escape",
                "return" => "enter",
                "del" => "delete",
                k => k,
            };
            mods.chain([k]).collect::<Vec<_>>().join("-")
        })
        .collect();
    let chords: Option<Vec<String>> = chords.iter().map(|c| canon(c)).collect();
    chords.filter(|c| !c.is_empty()).map(|c| c.join(" "))
}

fn default_bindings() -> Vec<Binding> {
    COMMANDS.iter().filter(|c| !c.key.is_empty()).filter_map(|cmd| Some(Binding { cmd, key: from_gpui(cmd.key)?, ctx: cmd.ctx })).collect()
}

/// A chord sequence ("cmd-k cmd-w") in canonical spelling.
fn from_gpui(key: &str) -> Option<String> {
    let chords: Option<Vec<String>> = key.split_whitespace().map(canon).collect();
    chords.map(|c| c.join(" "))
}

/// The defaults with the user's `keybindings.json` text applied; what it could not use comes back as warnings.
fn resolve(user: Option<&str>) -> (Vec<Binding>, Vec<String>) {
    let mut bindings = default_bindings();
    let mut warnings = Vec::new();
    let Some(text) = user else { return (bindings, warnings) };
    let entries: Vec<serde_json::Value> = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(e) => return (bindings, vec![format!("keybindings.json ignored: {e}")]),
    };
    for entry in entries {
        let get = |name| entry.get(name).and_then(|v| v.as_str());
        let (Some(command), key) = (get("command"), get("key").unwrap_or("")) else {
            warnings.push(format!("keybindings.json: entry without a command ignored: {entry}"));
            continue;
        };
        let (removing, id) = match command.strip_prefix('-') {
            Some(id) => (true, id),
            None => (false, command),
        };
        let Some(cmd) = COMMANDS.iter().find(|c| c.id == id) else {
            warnings.push(format!("keybindings.json: unknown command \"{id}\" ignored"));
            continue;
        };
        let ctx = match get("when").filter(|w| !w.is_empty()) {
            None => None,
            Some(w) => match Ctx::from_when(w) {
                Some(ctx) => Some(ctx),
                None => {
                    warnings.push(format!("keybindings.json: unknown context \"{w}\" in the rule for \"{command}\" ignored"));
                    continue;
                }
            },
        };
        if removing && key.is_empty() {
            bindings.retain(|b| b.cmd.id != id || ctx.is_some_and(|c| c != b.ctx));
            continue;
        }
        let Some(key) = from_vscode(key) else {
            warnings.push(format!("keybindings.json: \"{key}\" is not a key, the rule for \"{command}\" ignored"));
            continue;
        };
        if removing {
            bindings.retain(|b| !(b.cmd.id == id && b.key == key && ctx.is_none_or(|c| c == b.ctx)));
        } else {
            bindings.push(Binding { cmd, key, ctx: ctx.unwrap_or(Ctx::Global) });
        }
    }
    (bindings, warnings)
}

/// `~/.config/antler/keybindings.json`; `None` without a home directory.
pub(crate) fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/antler/keybindings.json"))
}

/// Binds the defaults and the user's file (a missing one is fine) over what is bound now; returns warnings to show.
/// Antler's bindings go after the editor's own, so they win at the same depth. The ones of an earlier load are
/// blanked first, so a removed key stops working (shortcut: it also blanks the editor's own binding of that key).
pub(crate) fn load(cx: &mut App, file: Option<&Path>) -> Vec<String> {
    let text = file.and_then(|f| std::fs::read_to_string(f).ok());
    let (new, warnings) = resolve(text.as_deref());
    let old = cx.try_global::<Active>().map(|a| a.0.clone()).unwrap_or_default();
    let predicate = |ctx: Ctx| ctx.predicate().and_then(|p| KeyBindingContextPredicate::parse(p).ok()).map(Rc::new);
    let blank = old.iter().map(|b| KeyBinding::load(&b.key, Box::new(NoAction {}), predicate(b.ctx), false, None, cx));
    let fresh = new.iter().map(|b| KeyBinding::load(&b.key, (b.cmd.action)(), predicate(b.ctx), false, None, cx));
    let all: Vec<KeyBinding> = blank.chain(fresh).filter_map(Result::ok).collect();
    cx.bind_keys(all);
    cx.set_global(Active(new));
    warnings
}

/// Shows `warnings` in the first window.
pub(crate) fn notify(cx: &mut App, warnings: Vec<String>) {
    let Some(window) = cx.windows().into_iter().next() else { return };
    for w in warnings {
        window.update(cx, |_, window, cx| window.push_notification(Notification::warning(w).title("Keybindings").autohide(false), cx)).ok();
    }
}

/// Reloads the file when it changes (checked once a second); the menus follow.
pub(crate) fn watch(cx: &mut App, file: PathBuf) {
    let stamp = |f: &Path| std::fs::metadata(f).and_then(|m| m.modified()).ok();
    cx.spawn(async move |cx| {
        let mut seen = stamp(&file);
        loop {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let now = stamp(&file);
            if now != seen {
                seen = now;
                cx.update(|cx| {
                    let warnings = load(cx, Some(&file));
                    set_menus(cx);
                    notify(cx, warnings);
                });
            }
        }
    })
    .detach();
}

/// A menu item for command `id`.
pub(crate) fn item(id: &str) -> MenuItem {
    let cmd = COMMANDS.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no command {id}"));
    MenuItem::Action { name: cmd.title.into(), action: (cmd.action)(), os_action: None, checked: false, disabled: false }
}

/// Like [`item`], with a check mark.
pub(crate) fn checked_item(id: &str, checked: bool) -> MenuItem {
    match item(id) {
        MenuItem::Action { name, action, os_action, disabled, .. } => MenuItem::Action { name, action, os_action, checked, disabled },
        other => other,
    }
}

/// "⌃⇧G" for the first binding of command `id`, in the user's current keys.
pub(crate) fn hint(cx: &App, id: &str) -> Option<String> {
    let key = cx.try_global::<Active>()?.0.iter().find(|b| b.cmd.id == id)?.key.clone();
    Some(key.split(' ').filter_map(parse).map(|k| pretty(&k)).collect::<Vec<_>>().join(" "))
}

/// "Commit (⌘↵)": the tooltip of command `id`; `text` alone if it has no key.
pub(crate) fn label(cx: &App, text: &str, id: &str) -> String {
    match hint(cx, id) {
        Some(h) => format!("{text} ({h})"),
        None => text.to_string(),
    }
}

fn pretty(k: &Keystroke) -> String {
    let m = &k.modifiers;
    let key = match k.key.as_str() {
        "enter" => "↵".to_string(),
        "backspace" => "⌫".into(),
        "escape" => "⎋".into(),
        "tab" => "⇥".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        k => k.to_uppercase(),
    };
    format!("{}{}{}{}{key}", if m.control { "⌃" } else { "" }, if m.alt { "⌥" } else { "" }, if m.shift { "⇧" } else { "" }, if m.platform { "⌘" } else { "" })
}

/// The menu bar, VS Code's menus with the commands Antler has: ids, "-" for a separator, "@…" for a submenu `set_menus` builds.
/// (Window and Help have nothing to hold yet: Antler has no window or help command.)
pub(crate) const MENUS: &[(&str, &[&str])] = &[
    (
        "File",
        &[
            "workbench.action.files.newUntitledFile",
            "-",
            "workbench.action.files.save",
            "workbench.action.files.saveAll",
            "-",
            "workbench.action.files.openFolder",
            "@recent",
            "workbench.action.closeFolder",
            "-",
            "workbench.action.closeActiveEditor",
            "workbench.action.reopenClosedEditor",
            "workbench.action.closeAllEditors",
        ],
    ),
    (
        "Edit",
        &[
            "undo",
            "redo",
            "-",
            "editor.action.clipboardCutAction",
            "editor.action.clipboardCopyAction",
            "editor.action.clipboardPasteAction",
            "-",
            "actions.find",
            "editor.action.startFindReplace",
            "editor.action.nextMatchFindAction",
            "editor.action.previousMatchFindAction",
            "workbench.view.search",
            "-",
            "editor.action.commentLine",
            "editor.action.blockComment",
        ],
    ),
    (
        "Selection",
        &[
            "editor.action.selectAll",
            "-",
            "editor.action.copyLinesUpAction",
            "editor.action.copyLinesDownAction",
            "editor.action.moveLinesUpAction",
            "editor.action.moveLinesDownAction",
            "-",
            "editor.action.addSelectionToNextFindMatch",
            "editor.action.selectHighlights",
            "editor.action.insertCursorAtEndOfEachLineSelected",
        ],
    ),
    (
        "View",
        &[
            "workbench.view.explorer",
            "workbench.view.search",
            "workbench.view.scm",
            "-",
            "workbench.action.toggleSidebarVisibility",
            "markdown.showPreview",
            "editor.action.toggleWordWrap",
            "-",
            "editor.action.fontZoomIn",
            "editor.action.fontZoomOut",
            "editor.action.fontZoomReset",
            "@minimap",
        ],
    ),
    (
        "Go",
        &[
            "workbench.action.quickOpen",
            "-",
            "workbench.action.nextEditor",
            "workbench.action.previousEditor",
            "workbench.action.openNextRecentlyUsedEditor",
            "workbench.action.openPreviousRecentlyUsedEditor",
            "-",
            "workbench.action.editor.nextChange",
            "workbench.action.editor.previousChange",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test; // gpui_kit's glob brings its own `test` macro

    #[test]
    fn no_key_is_bound_to_two_commands_in_the_same_context() {
        let mut seen: Vec<(String, Ctx, &str)> = Vec::new();
        for b in default_bindings() {
            if let Some((_, _, id)) = seen.iter().find(|(k, c, _)| *k == b.key && *c == b.ctx) {
                assert_eq!(*id, b.cmd.id, "{} is bound twice in {:?}", b.key, b.ctx);
            }
            seen.push((b.key, b.ctx, b.cmd.id));
        }
    }

    #[test]
    fn every_default_key_and_context_is_valid() {
        for c in COMMANDS {
            assert!(c.key.is_empty() || from_gpui(c.key).is_some(), "{} has a bad key {}", c.id, c.key);
            assert!(c.ctx.predicate().is_none_or(|p| KeyBindingContextPredicate::parse(p).is_ok()), "{}: bad context", c.id);
        }
    }

    #[test]
    fn a_command_has_one_title_whatever_its_keys() {
        for c in COMMANDS {
            assert!(COMMANDS.iter().filter(|o| o.id == c.id).all(|o| o.title == c.title), "{} has two titles", c.id);
        }
    }

    #[test]
    fn the_menus_hold_only_commands_of_the_table() {
        for (_, ids) in MENUS {
            for id in ids.iter().filter(|i| **i != "-" && !i.starts_with('@')) {
                assert!(COMMANDS.iter().any(|c| c.id == *id), "menu item {id} is not a command");
            }
        }
    }

    #[test]
    fn vscode_keys_become_gpui_keys() {
        assert_eq!(from_vscode("cmd+shift+f").as_deref(), Some("cmd-shift-f"));
        assert_eq!(from_vscode("Ctrl+K Ctrl+S").as_deref(), Some("ctrl-k ctrl-s"));
        assert_eq!(from_vscode("meta+enter").as_deref(), Some("cmd-enter"));
        assert_eq!(from_vscode("cmd+=").as_deref(), Some("cmd-="));
        assert_eq!(from_vscode("cmd+-").as_deref(), Some("cmd--"));
        assert_eq!(from_vscode(""), None);
    }

    fn keys_of<'a>(b: &'a [Binding], id: &str) -> Vec<(&'a str, Ctx)> {
        b.iter().filter(|b| b.cmd.id == id).map(|b| (b.key.as_str(), b.ctx)).collect()
    }

    #[test]
    fn a_rule_rebinds_and_a_minus_command_removes() {
        let user = r#"[
            { "key": "cmd+b", "command": "-workbench.action.toggleSidebarVisibility" },
            { "key": "cmd+j", "command": "workbench.action.toggleSidebarVisibility", "when": "editor" },
            { "key": "cmd+d", "command": "-editor.action.addSelectionToNextFindMatch" },
            { "key": "", "command": "-editor.action.selectHighlights" }
        ]"#;
        let (b, warnings) = resolve(Some(user));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(keys_of(&b, "workbench.action.toggleSidebarVisibility"), [("cmd-j", Ctx::Editor)]);
        assert!(keys_of(&b, "editor.action.addSelectionToNextFindMatch").is_empty());
        assert!(keys_of(&b, "editor.action.selectHighlights").is_empty());
        assert!(!keys_of(&b, "workbench.action.files.save").is_empty());
    }

    #[test]
    fn what_cannot_be_used_is_ignored_with_a_warning() {
        let user = r#"[
            { "key": "cmd+k", "command": "no.such.command" },
            { "key": "cmd+k", "command": "workbench.action.files.save", "when": "editorTextFocus && foo" },
            { "key": "nonsense+nokey", "command": "workbench.action.files.save" },
            { "command": 3 }
        ]"#;
        let (b, warnings) = resolve(Some(user));
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert_eq!(b.len(), default_bindings().len());
        let (b, warnings) = resolve(Some("not json"));
        assert_eq!((b.len(), warnings.len()), (default_bindings().len(), 1));
    }

    #[test]
    fn keys_show_as_on_a_mac() {
        let k = |s| pretty(&Keystroke::parse(s).unwrap());
        assert_eq!(k("cmd-enter"), "⌘↵");
        assert_eq!(k("ctrl-shift-g"), "⌃⇧G");
        assert_eq!(k("alt-f5"), "⌥F5");
    }
}
