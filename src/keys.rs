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
use crate::theme_view::{InstallThemeFromVsix, SelectIconTheme, SelectTheme};
use crate::goto_view::{CloseGoToFile, CommandPalette, GoToFile, GoToLine};
use crate::minimap_view::{ToggleMinimap, ToggleRenderCharacters};
use crate::stash_view::CloseStash;
use crate::tabs::*;
use crate::ports_view::{ForwardPort, ShowPorts, StopForwardingPort};
use crate::remote_view::{CloseRemoteConnection, ConnectToHost};
use crate::{CheckForUpdates, CloseFolder, CloseTab, CloseWindow, Commit, FocusNextWindow, MinimizeWindow, NewWindow, OpenFolder, OpenKeyboardShortcuts, OpenSettings, Quit, Save, SaveAll, ShowExplorer, ShowGit, ShowSearch, ToggleBlameEditorDecoration, ToggleBlameStatusBar, ToggleSidebar, set_menus};

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

    /// The `when` of keybindings.json for this context; `None` for Global and for the contexts it does not accept.
    fn when_name(self) -> Option<&'static str> {
        Some(match self {
            Ctx::Input => "input",
            Ctx::Editor => "editor",
            Ctx::Explorer => "explorer",
            Ctx::Git => "git",
            Ctx::Search => "search",
            Ctx::Commit => "commit",
            _ => return None,
        })
    }

    /// Whether a keybindings.json rule can bind a key here.
    pub fn editable(self) -> bool {
        self == Ctx::Global || self.when_name().is_some()
    }

    /// The When column: empty for Global.
    pub fn label(self) -> &'static str {
        self.when_name().or(self.predicate()).unwrap_or("")
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

impl Cmd {
    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }
}

/// What the Command palette lists: each command once, minus those that cannot run from it. Commands of a
/// text field need an active editor; the ones of the Explorer, Git view and pickers act on a selection or
/// an open popup, so they are left out (VS Code hides commands whose precondition fails).
pub(crate) fn palette_commands(has_editor: bool) -> Vec<&'static Cmd> {
    let mut seen = std::collections::HashSet::new();
    let ok = |c: &Cmd| match c.ctx {
        Ctx::Global => true,
        Ctx::Input | Ctx::Editor => has_editor,
        _ => false,
    };
    COMMANDS.iter().filter(|c| ok(c) && seen.insert(c.id)).collect()
}

macro_rules! cmds {
    ($(($id:literal, $title:expr, $key:literal, $ctx:ident, $action:expr)),* $(,)?) => {
        pub(crate) static COMMANDS: &[Cmd] = &[$(Cmd { id: $id, title: $title, key: $key, ctx: Ctx::$ctx, action: || Box::new($action) }),*];
    };
}

// A key bound twice, in the same context, to two commands is a test failure. Several rows with one id are
// several keys (or contexts) of one command. Rows with no key are bound by gpui-component itself or by nothing.
cmds![
    // File
    ("workbench.action.files.newUntitledFile", "New File", "cmd-n", Global, NewUntitledFile),
    ("workbench.action.newWindow", "New Window", "shift-cmd-n", Global, NewWindow),
    ("workbench.action.files.save", "Save", "cmd-s", Global, Save),
    ("workbench.action.files.saveAll", "Save All", "alt-cmd-s", Global, SaveAll),
    ("workbench.action.files.openFolder", "Open Folder…", "cmd-o", Global, OpenFolder),
    ("workbench.action.closeFolder", "Close Folder", "cmd-k f", Global, CloseFolder),
    ("remote.ssh.connect", "Connect to Host…", "", Global, ConnectToHost),
    ("workbench.action.remote.close", "Close Remote Connection", "", Global, CloseRemoteConnection),
    ("git.blame.toggleStatusBarItem", "Git: Toggle Git Blame Status Bar Item", "", Global, ToggleBlameStatusBar),
    ("git.blame.toggleEditorDecoration", "Git: Toggle Git Blame Editor Decoration", "", Global, ToggleBlameEditorDecoration),
    ("remote.tunnel.forwardCommandPalette", "Forward a Port", "", Global, ForwardPort),
    ("remote.tunnel.closeCommandPalette", "Stop Forwarding Port", "", Global, StopForwardingPort),
    ("~remote.forwardedPorts.focus", "Ports: Focus on Ports View", "", Global, ShowPorts),
    ("workbench.action.closeWindow", "Close Window", "shift-cmd-w", Global, CloseWindow),
    ("antler.minimizeWindow", "Minimize", "cmd-m", Global, MinimizeWindow),
    ("antler.focusNextWindow", "Next Window", "cmd-`", Global, FocusNextWindow),
    ("workbench.action.closeActiveEditor", "Close Editor", "cmd-w", Global, CloseTab),
    ("workbench.action.reopenClosedEditor", "Reopen Closed Editor", "shift-cmd-t", Global, ReopenClosedTab),
    ("workbench.action.closeAllEditors", "Close All Editors", "cmd-k cmd-w", Global, CloseAllTabs),
    ("workbench.action.closeOtherEditors", "Close Other Editors", "alt-cmd-t", Global, CloseOtherTabs),
    ("workbench.action.closeUnmodifiedEditors", "Close Saved Editors", "cmd-k u", Global, CloseSavedTabs),
    ("antler.closeEditorsToTheRight", "Close Editors to the Right", "", Global, CloseTabsToTheRight),
    ("workbench.action.closeEditorsInGroup", "Close All Editors in Group", "cmd-k w", Global, CloseEditorsInGroup),
    ("workbench.action.keepEditor", "Keep Open", "cmd-k enter", Global, KeepTabOpen),
    ("workbench.action.quit", "Quit Antler", "cmd-q", Global, Quit),
    ("antler.checkForUpdates", "Check for Updates…", "", Global, CheckForUpdates),
    ("workbench.action.openSettings", "Settings…", "cmd-,", Global, OpenSettings),
    ("workbench.action.openGlobalKeybindings", "Keyboard Shortcuts", "cmd-k cmd-s", Global, OpenKeyboardShortcuts),
    ("workbench.action.selectTheme", "Color Theme", "cmd-k cmd-t", Global, SelectTheme),
    ("workbench.action.selectIconTheme", "File Icon Theme", "", Global, SelectIconTheme),
    ("antler.installThemeFromVsix", "Install Theme from VSIX…", "", Global, InstallThemeFromVsix),
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
    ("workbench.action.showCommands", "Command Palette…", "shift-cmd-p", Global, CommandPalette),
    ("workbench.view.explorer", "Explorer", "shift-cmd-e", Global, ShowExplorer),
    ("workbench.view.scm", "Git", "ctrl-shift-g", Global, ShowGit),
    ("workbench.action.toggleSidebarVisibility", "Toggle Sidebar", "cmd-b", Global, ToggleSidebar),
    ("markdown.showPreview", "Toggle Markdown Preview", "cmd-shift-v", Global, TogglePreview),
    ("editor.action.toggleWordWrap", "Toggle Word Wrap", "alt-z", Editor, ToggleWordWrap),
    ("editor.action.fontZoomIn", "Zoom In", "cmd-=", Editor, ZoomIn),
    ("editor.action.fontZoomIn", "Zoom In", "cmd-+", Editor, ZoomIn), // layouts with a + key of its own (Italian): VS Code binds both
    ("editor.action.fontZoomOut", "Zoom Out", "cmd--", Editor, ZoomOut),
    ("editor.action.fontZoomReset", "Reset Zoom", "cmd-0", Editor, ZoomReset),
    // VS Code's own keys; Split Editor Down has none by default (Split Editor Orthogonal, cmd-k cmd-\, splits across the current row)
    ("workbench.action.splitEditor", "Split Editor Right", "cmd-\\", Global, SplitEditor),
    ("workbench.action.splitEditorDown", "Split Editor Down", "", Global, SplitEditorDown),
    ("workbench.action.splitEditorOrthogonal", "Split Editor Orthogonal", "cmd-k cmd-\\", Global, SplitEditorOrthogonal),
    ("antler.toggleMinimap", "Show Minimap", "", Global, ToggleMinimap),
    ("antler.toggleMinimapCharacters", "Render Characters", "", Global, ToggleRenderCharacters),
    // Go
    ("workbench.action.quickOpen", "Go to File…", "cmd-p", Global, GoToFile),
    ("workbench.action.gotoLine", "Go to Line…", "ctrl-g", Global, GoToLine),
    ("workbench.action.focusFirstEditorGroup", "Focus First Editor Group", "cmd-1", Global, FocusFirstGroup),
    ("workbench.action.focusSecondEditorGroup", "Focus Second Editor Group", "cmd-2", Global, FocusSecondGroup),
    ("workbench.action.focusThirdEditorGroup", "Focus Third Editor Group", "cmd-3", Global, FocusThirdGroup),
    ("workbench.action.focusFourthEditorGroup", "Focus Fourth Editor Group", "cmd-4", Global, FocusFourthGroup),
    ("workbench.action.focusFifthEditorGroup", "Focus Fifth Editor Group", "cmd-5", Global, FocusFifthGroup),
    ("workbench.action.focusSixthEditorGroup", "Focus Sixth Editor Group", "cmd-6", Global, FocusSixthGroup),
    ("workbench.action.focusSeventhEditorGroup", "Focus Seventh Editor Group", "cmd-7", Global, FocusSeventhGroup),
    ("workbench.action.focusEighthEditorGroup", "Focus Eighth Editor Group", "cmd-8", Global, FocusEighthGroup),
    ("workbench.action.nextEditor", "Next Editor", "alt-cmd-right", Global, NextTab),
    ("workbench.action.previousEditor", "Previous Editor", "alt-cmd-left", Global, PreviousTab),
    ("workbench.action.openNextRecentlyUsedEditor", "Next Used Editor", "ctrl-tab", Global, NextRecentTab),
    ("workbench.action.openPreviousRecentlyUsedEditor", "Previous Used Editor", "ctrl-shift-tab", Global, PreviousRecentTab),
    ("workbench.action.editor.nextChange", "Next Change", "alt-f5", Global, NextChange),
    ("workbench.action.editor.previousChange", "Previous Change", "shift-alt-f5", Global, PreviousChange),
    // the active file (VS Code's cmd-k chords, which work anywhere), and the selected row of the Explorer or Git view
    ("workbench.action.files.copyPathOfActiveFile", "Copy Path of Active File", "cmd-k alt-cmd-c", Global, CopyTabPath),
    ("workbench.action.files.copyRelativePathOfActiveFile", "Copy Relative Path of Active File", "cmd-k shift-alt-cmd-c", Global, CopyRelativeTabPath),
    ("workbench.action.files.revealActiveFileInWindows", crate::platform::REVEAL_ACTIVE_FILE_IN_OS, "cmd-k r", Global, RevealTabInFinder),
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
    /// From the user's keybindings.json, not the table.
    user: bool,
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
    defaults(!crate::platform::MAC)
}

/// The table's keys (the Mac's), or with `windows` VS Code's Windows/Linux defaults (see [`windows_key`]).
fn defaults(windows: bool) -> Vec<Binding> {
    let key = |cmd: &Cmd| if windows { windows_key(cmd.id, cmd.key) } else { cmd.key.to_string() };
    COMMANDS.iter().map(|cmd| (cmd, key(cmd))).filter(|(_, k)| !k.is_empty()).filter_map(|(cmd, k)| Some(Binding { cmd, key: from_gpui(&k)?, ctx: cmd.ctx, user: false })).collect()
}

/// Commands whose Windows key in VS Code is not the Mac one with Ctrl for Cmd ("" = no key there).
const WINDOWS_KEYS: &[(&str, &str)] = &[
    ("workbench.action.files.saveAll", "ctrl-k s"),
    ("workbench.action.files.openFolder", "ctrl-k ctrl-o"),
    ("antler.minimizeWindow", ""),
    ("antler.focusNextWindow", ""),
    ("workbench.action.closeOtherEditors", ""),
    ("editor.action.startFindReplace", "ctrl-h"),
    ("editor.action.nextMatchFindAction", "f3"),
    ("editor.action.previousMatchFindAction", "shift-f3"),
    ("editor.unfold", "ctrl-shift-]"),
    ("workbench.action.nextEditor", "ctrl-pagedown"),
    ("workbench.action.previousEditor", "ctrl-pageup"),
    ("workbench.action.files.copyPathOfActiveFile", "shift-alt-c"),
    ("workbench.action.files.copyRelativePathOfActiveFile", "ctrl-k ctrl-shift-c"),
    ("copyFilePath", "shift-alt-c"),
    ("copyRelativeFilePath", "ctrl-k ctrl-shift-c"),
    ("renameFile", "f2"),
    ("moveFileToTrash", "delete"),
    ("antler.explorer.redo", "ctrl-y"),
];

/// The Windows default of a command whose Mac key is `mac`: VS Code's own where it differs ([`WINDOWS_KEYS`]),
/// otherwise Ctrl in place of Cmd (GPUI's `cmd` is the Windows key there).
fn windows_key(id: &str, mac: &str) -> String {
    match WINDOWS_KEYS.iter().find(|(i, _)| *i == id) {
        Some((_, k)) => k.to_string(),
        None => ctrl_for_cmd(mac),
    }
}

/// "cmd-k cmd-s" as "ctrl-k ctrl-s".
fn ctrl_for_cmd(key: &str) -> String {
    key.split(' ').map(|chord| chord.split('-').map(|m| if m == "cmd" { "ctrl" } else { m }).collect::<Vec<_>>().join("-")).collect::<Vec<_>>().join(" ")
}

/// A Mac key with its primary modifier as this OS has it: Cmd on macOS, Ctrl elsewhere.
fn primary(mac: &str) -> String {
    if crate::platform::MAC { mac.to_string() } else { ctrl_for_cmd(mac) }
}

/// The primary modifier as the labels show it, in front of a key: "⌘" on macOS ("⌘S"), "Ctrl+" elsewhere
/// ("Ctrl+S").
pub(crate) fn primary_modifier() -> String {
    let label = pretty_key(&primary("cmd-a"));
    label.strip_suffix('A').unwrap_or(&label).to_string()
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
            bindings.push(Binding { cmd, key, ctx: ctx.unwrap_or(Ctx::Global), user: true });
        }
    }
    (bindings, warnings)
}

/// `~/.config/antler/keybindings.json`; `None` without a home directory.
pub(crate) fn default_path() -> Option<PathBuf> {
    crate::paths::config("keybindings.json")
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
    Some(pretty_key(&key))
}

/// "⌘K ⌘S" for "cmd-k cmd-s".
pub(crate) fn pretty_key(key: &str) -> String {
    key.split(' ').filter_map(parse).map(|k| pretty(&k)).collect::<Vec<_>>().join(" ")
}

/// "Commit (⌘↵)": the tooltip of command `id`; `text` alone if it has no key.
pub(crate) fn label(cx: &App, text: &str, id: &str) -> String {
    match hint(cx, id) {
        Some(h) => format!("{text} ({h})"),
        None => text.to_string(),
    }
}

fn pretty(k: &Keystroke) -> String {
    if crate::platform::MAC { pretty_mac(k) } else { pretty_words(k) }
}

/// VS Code's labels on Windows and Linux:"Ctrl+Shift+G", "Ctrl+K Ctrl+S" (chords joined by the caller).
fn pretty_words(k: &Keystroke) -> String {
    let m = &k.modifiers;
    let key = match k.key.as_str() {
        "up" => "UpArrow".to_string(),
        "down" => "DownArrow".into(),
        "left" => "LeftArrow".into(),
        "right" => "RightArrow".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        k => {
            // Enter, Escape, Backspace, Delete, Tab, F3; a single letter is upper case
            let mut chars = k.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    };
    let mods = [(m.control, "Ctrl"), (m.shift, "Shift"), (m.alt, "Alt"), (m.platform, "Win")];
    mods.iter().filter(|(on, _)| *on).map(|(_, n)| *n).chain([key.as_str()]).collect::<Vec<_>>().join("+")
}

fn pretty_mac(k: &Keystroke) -> String {
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

// ---- the Keyboard Shortcuts editor: rows, search, conflicts and the edits of keybindings.json ----

/// One line of the Keyboard Shortcuts table: a key of a command, or a command with no key (`key` empty).
#[derive(Clone)]
pub(crate) struct Row {
    pub cmd: &'static Cmd,
    /// GPUI's spelling, canonical; empty without a key.
    pub key: String,
    pub ctx: Ctx,
    pub user: bool,
}

impl Row {
    pub fn source(&self) -> &'static str {
        if self.user { "User" } else { "Default" }
    }
}

/// Every command of the table once in table order, one row per key it has, or one without a key.
fn rows_of(bindings: &[Binding]) -> Vec<Row> {
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for cmd in COMMANDS.iter().filter(|c| seen.insert(c.id)) {
        let own: Vec<_> = bindings.iter().filter(|b| b.cmd.id == cmd.id).collect();
        if own.is_empty() {
            rows.push(Row { cmd, key: String::new(), ctx: cmd.ctx, user: false });
        }
        rows.extend(own.into_iter().map(|b| Row { cmd, key: b.key.clone(), ctx: b.ctx, user: b.user }));
    }
    rows
}

/// The rows for the keys in force now.
pub(crate) fn rows(cx: &App) -> Vec<Row> {
    rows_of(&cx.try_global::<Active>().map(|a| a.0.clone()).unwrap_or_default())
}

/// The rows whose title, command id or key (as GPUI, VS Code or the Mac spells it) contain `query`.
pub(crate) fn filter(rows: &[Row], query: &str) -> Vec<Row> {
    let q = query.trim().to_lowercase();
    let hit = |r: &Row| [r.cmd.title.to_string(), r.cmd.id.to_string(), r.key.clone(), to_vscode(&r.key), pretty_key(&r.key)].iter().any(|t| t.to_lowercase().contains(&q));
    rows.iter().filter(|r| hit(r)).cloned().collect()
}

/// Titles of the other commands that already have `key` in context `ctx`.
pub(crate) fn conflicts(rows: &[Row], id: &str, key: &str, ctx: Ctx) -> Vec<&'static str> {
    rows.iter().filter(|r| r.key == key && r.ctx == ctx && r.cmd.id != id).map(|r| r.cmd.title).collect()
}

/// GPUI's "cmd-shift-f ctrl-k" as VS Code's "ctrl+shift+cmd+f ctrl+k".
fn to_vscode(key: &str) -> String {
    let chord = |k: Keystroke| {
        let m = &k.modifiers;
        let mods = [(m.control, "ctrl"), (m.shift, "shift"), (m.alt, "alt"), (m.platform, "cmd")];
        mods.iter().filter(|(on, _)| *on).map(|(_, n)| *n).chain([k.key.as_str()]).collect::<Vec<_>>().join("+")
    };
    key.split_whitespace().filter_map(parse).map(chord).collect::<Vec<_>>().join(" ")
}

fn entries(text: &str) -> Result<Vec<serde_json::Value>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|e| format!("keybindings.json is not a list of rules: {e}"))
}

fn rule(id: &str, key: &str, ctx: Ctx) -> serde_json::Value {
    let mut r = serde_json::json!({ "key": to_vscode(key), "command": id });
    if let Some(w) = ctx.when_name() {
        r["when"] = w.into();
    }
    r
}

fn render(entries: Vec<serde_json::Value>) -> Result<String, String> {
    serde_json::to_string_pretty(&entries).map(|t| t + "\n").map_err(|e| e.to_string())
}

/// Takes the key of `row` away: its own rule goes, or a default one gets a `-command` rule.
fn drop_key(entries: &mut Vec<serde_json::Value>, row: &Row) {
    if row.key.is_empty() {
        return;
    }
    if row.user {
        entries.retain(|e| {
            let get = |n| e.get(n).and_then(|v| v.as_str());
            let ctx = get("when").and_then(Ctx::from_when).unwrap_or(Ctx::Global);
            !(get("command") == Some(row.cmd.id) && ctx == row.ctx && get("key").and_then(from_vscode).as_deref() == Some(&row.key))
        });
    } else {
        entries.push(rule(&format!("-{}", row.cmd.id), &row.key, row.ctx));
    }
}

/// keybindings.json `text` with `row`'s key changed to `new` (GPUI's spelling): the old key out, the new one in.
pub(crate) fn change_key(text: &str, row: &Row, new: &str) -> Result<String, String> {
    let mut e = entries(text)?;
    if new != row.key {
        drop_key(&mut e, row);
        e.push(rule(row.cmd.id, new, row.ctx));
    }
    render(e)
}

/// keybindings.json `text` without `row`'s key.
pub(crate) fn remove_key(text: &str, row: &Row) -> Result<String, String> {
    let mut e = entries(text)?;
    drop_key(&mut e, row);
    render(e)
}

/// keybindings.json `text` without any rule of command `id`: its default keys are back.
pub(crate) fn reset_command(text: &str, id: &str) -> Result<String, String> {
    let mut e = entries(text)?;
    e.retain(|r| r.get("command").and_then(|c| c.as_str()).is_none_or(|c| c.strip_prefix('-').unwrap_or(c) != id));
    render(e)
}

/// Rewrites the user's keybindings.json with `edit(old text)` and applies it at once (the menus and tooltips too).
pub(crate) fn update_file(cx: &mut App, edit: impl FnOnce(&str) -> Result<String, String>) {
    let Some(file) = default_path() else { return };
    let result = edit(&std::fs::read_to_string(&file).unwrap_or_default()).and_then(|text| {
        std::fs::create_dir_all(file.parent().unwrap_or(&file)).map_err(|e| e.to_string())?;
        std::fs::write(&file, text).map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => {
            let warnings = load(cx, Some(&file));
            set_menus(cx);
            notify(cx, warnings);
        }
        Err(e) => notify(cx, vec![e]),
    }
}

/// The keybindings.json path, created as `[]` if missing.
pub(crate) fn ensure_file() -> Option<PathBuf> {
    let file = default_path()?;
    if !file.exists() {
        std::fs::create_dir_all(file.parent()?).ok()?;
        std::fs::write(&file, "[]\n").ok()?;
    }
    Some(file)
}

/// The menu bar, VS Code's menus with the commands Antler has: ids, "-" for a separator, "@…" for a submenu `set_menus` builds.
/// (Help has nothing to hold yet.)
pub(crate) const MENUS: &[(&str, &[&str])] = &[
    (
        "File",
        &[
            "workbench.action.files.newUntitledFile",
            "workbench.action.newWindow",
            "-",
            "workbench.action.files.save",
            "workbench.action.files.saveAll",
            "-",
            "workbench.action.files.openFolder",
            "@recent",
            "remote.ssh.connect",
            "workbench.action.closeFolder",
            "workbench.action.closeWindow",
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
            "workbench.action.showCommands",
            "-",
            "workbench.view.explorer",
            "workbench.view.search",
            "workbench.view.scm",
            "-",
            "workbench.action.toggleSidebarVisibility",
            "markdown.showPreview",
            "editor.action.toggleWordWrap",
            "@layout",
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
            "workbench.action.gotoLine",
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
    ("Window", &["antler.minimizeWindow", "antler.focusNextWindow", "-", "@windows"]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test; // gpui_kit's glob brings its own `test` macro

    #[test]
    fn no_key_is_bound_to_two_commands_in_the_same_context() {
        for windows in [false, true] {
            let mut seen: Vec<(String, Ctx, &str)> = Vec::new();
            for b in defaults(windows) {
                if let Some((_, _, id)) = seen.iter().find(|(k, c, _)| *k == b.key && *c == b.ctx) {
                    assert_eq!(*id, b.cmd.id, "{} is bound twice in {:?} (windows: {windows})", b.key, b.ctx);
                }
                seen.push((b.key, b.ctx, b.cmd.id));
            }
        }
    }

    fn default_key(windows: bool, id: &str) -> Vec<String> {
        defaults(windows).into_iter().filter(|b| b.cmd.id == id).map(|b| b.key).collect()
    }

    #[test]
    fn windows_takes_ctrl_for_cmd_and_vs_codes_own_keys_where_they_differ() {
        assert_eq!(default_key(true, "workbench.action.files.save"), ["ctrl-s"]);
        assert_eq!(default_key(true, "workbench.action.closeAllEditors"), ["ctrl-k ctrl-w"]);
        assert_eq!(default_key(true, "editor.action.fontZoomOut"), ["ctrl--"]);
        assert_eq!(default_key(true, "workbench.view.scm"), ["ctrl-shift-g"], "already Ctrl on the Mac");
        assert_eq!(default_key(true, "renameFile"), ["f2"]);
        assert_eq!(default_key(true, "moveFileToTrash"), ["delete"]);
        assert_eq!(default_key(true, "editor.action.nextMatchFindAction"), ["f3"], "ctrl-g is Go to Line");
        assert!(default_key(true, "antler.focusNextWindow").is_empty(), "no default key on Windows");
        assert_eq!(default_key(false, "workbench.action.files.save"), ["cmd-s"], "the Mac keeps the table");
        assert!(defaults(true).iter().all(|b| !b.key.contains("cmd")), "GPUI's cmd is the Windows key");
    }

    #[test]
    fn windows_labels_spell_the_keys_out_like_vs_code() {
        let label = |k: &str| pretty_words(&parse(k).unwrap());
        assert_eq!(label("ctrl-shift-g"), "Ctrl+Shift+G");
        assert_eq!(label("f3"), "F3");
        assert_eq!(label("ctrl-pagedown"), "Ctrl+PageDown");
        assert_eq!(label("shift-alt-up"), "Shift+Alt+UpArrow");
        assert_eq!(label("ctrl-enter"), "Ctrl+Enter");
        assert_eq!(label("ctrl-ñandú"), "Ctrl+Ñandú", "a key name starting with a multibyte letter");
    }

    #[test]
    fn the_primary_modifier_reads_like_the_labels() {
        assert_eq!(primary("cmd-k cmd-s"), if crate::platform::MAC { "cmd-k cmd-s" } else { "ctrl-k ctrl-s" });
        assert_eq!(primary_modifier(), if crate::platform::MAC { "⌘" } else { "Ctrl+" });
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
            { "key": "PRIMARY+b", "command": "-workbench.action.toggleSidebarVisibility" },
            { "key": "cmd+j", "command": "workbench.action.toggleSidebarVisibility", "when": "editor" },
            { "key": "PRIMARY+d", "command": "-editor.action.addSelectionToNextFindMatch" },
            { "key": "", "command": "-editor.action.selectHighlights" }
        ]"#.replace("PRIMARY", primary("cmd").as_str());
        let (b, warnings) = resolve(Some(&user));
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

    /// The default row of `id` whose Mac key is `key` (its Windows key off macOS).
    fn row(id: &str, key: &str) -> Row {
        let key = if crate::platform::MAC { key.to_string() } else { windows_key(id, key) };
        rows_of(&default_bindings()).into_iter().find(|r| r.cmd.id == id && r.key == key).unwrap()
    }

    #[test]
    fn changing_a_default_key_removes_the_old_one_and_adds_the_new() {
        let text = change_key("", &row("workbench.action.toggleSidebarVisibility", "cmd-b"), "cmd-j").unwrap();
        let (b, w) = resolve(Some(&text));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(keys_of(&b, "workbench.action.toggleSidebarVisibility"), [("cmd-j", Ctx::Global)]);
        assert!(text.contains("\"-workbench.action.toggleSidebarVisibility\"") && text.contains("cmd+j"));
    }

    #[test]
    fn changing_a_user_key_replaces_its_rule_and_keeps_the_when() {
        let one = change_key("", &row("editor.action.deleteLines", "cmd-shift-k"), "ctrl-d").unwrap();
        let rows = rows_of(&resolve(Some(&one)).0);
        let mine = rows.iter().find(|r| r.cmd.id == "editor.action.deleteLines").unwrap();
        assert!(mine.user && mine.ctx == Ctx::Editor && mine.key == "ctrl-d");
        let two = change_key(&one, mine, "ctrl-e").unwrap();
        let (b, _) = resolve(Some(&two));
        assert_eq!(keys_of(&b, "editor.action.deleteLines"), [("ctrl-e", Ctx::Editor)]);
        assert_eq!(entries(&two).unwrap().len(), 2); // the -command and the new rule, no leftover ctrl-d
    }

    #[test]
    fn remove_and_reset() {
        let text = remove_key("", &row("workbench.action.files.save", "cmd-s")).unwrap();
        assert!(keys_of(&resolve(Some(&text)).0, "workbench.action.files.save").is_empty());
        let rows = rows_of(&resolve(Some(&text)).0);
        assert!(rows.iter().any(|r| r.cmd.id == "workbench.action.files.save" && r.key.is_empty())); // the command stays listed
        let text = change_key(&text, rows.iter().find(|r| r.cmd.id == "workbench.action.files.save").unwrap(), "cmd-k cmd-s").unwrap();
        assert_eq!(keys_of(&resolve(Some(&text)).0, "workbench.action.files.save"), [("cmd-k cmd-s", Ctx::Global)]);
        let text = reset_command(&text, "workbench.action.files.save").unwrap();
        assert_eq!(keys_of(&resolve(Some(&text)).0, "workbench.action.files.save"), [(primary("cmd-s").as_str(), Ctx::Global)]);
        assert!(change_key("not json", &row("workbench.action.files.save", "cmd-s"), "cmd-j").is_err());
    }

    #[test]
    fn a_key_in_use_is_reported_in_the_same_context_only() {
        let rows = rows_of(&default_bindings());
        assert_eq!(conflicts(&rows, "workbench.action.files.saveAll", &primary("cmd-s"), Ctx::Global), ["Save"]);
        assert!(conflicts(&rows, "workbench.action.files.save", &primary("cmd-s"), Ctx::Global).is_empty(), "its own key");
        assert!(conflicts(&rows, "workbench.action.files.saveAll", &primary("cmd-s"), Ctx::Editor).is_empty(), "another context");
    }

    #[test]
    fn search_matches_title_id_and_key() {
        let rows = rows_of(&default_bindings());
        let ids = |q| filter(&rows, q).iter().map(|r| r.cmd.id).collect::<Vec<_>>();
        assert!(ids("toggle sidebar").contains(&"workbench.action.toggleSidebarVisibility"));
        assert!(ids("openGlobalKey").contains(&"workbench.action.openGlobalKeybindings"));
        let (vscode, label) = if crate::platform::MAC { ("cmd+b", "⌘B") } else { ("ctrl+b", "Ctrl+B") };
        assert!(ids(vscode).contains(&"workbench.action.toggleSidebarVisibility"));
        assert!(ids(label).contains(&"workbench.action.toggleSidebarVisibility"));
        assert_eq!(filter(&rows, "").len(), rows.len());
    }

    #[test]
    fn keys_round_trip_through_vscode_spelling() {
        for k in ["cmd-shift-f", "cmd-k cmd-s", "cmd--", "ctrl-shift-tab", "cmd-+"] {
            assert_eq!(from_vscode(&to_vscode(k)).as_deref(), Some(k), "{k}");
        }
    }

    #[test]
    fn keys_show_as_on_a_mac() {
        let k = |s| pretty_mac(&Keystroke::parse(s).unwrap());
        assert_eq!(k("cmd-enter"), "⌘↵");
        assert_eq!(k("ctrl-shift-g"), "⌃⇧G");
        assert_eq!(k("alt-f5"), "⌥F5");
    }
}
