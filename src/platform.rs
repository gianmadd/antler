//! The one place where the app's words and layout follow the OS, like VS Code's per-OS defaults: macOS has Cmd
//! keys shown as symbols (⌘S), an app menu, Finder and the Trash; every other OS has VS Code's Windows/Linux
//! layout: Ctrl keys spelled out (Ctrl+S), the menus in the window, File Explorer and the Recycle Bin.
//! The key tables themselves are in `keys`, which reads [`MAC`].

/// VS Code's macOS layout; `false` on Windows (and Linux), which share the Ctrl-based one.
pub(crate) const MAC: bool = cfg!(target_os = "macos");

/// `mac` on macOS, `other` elsewhere.
const fn words(mac: &'static str, other: &'static str) -> &'static str {
    if MAC { mac } else { other }
}

/// Where a deleted file goes ("Move to {TRASH}").
pub(crate) const TRASH: &str = words("Trash", "Recycle Bin");
/// The context menus' item that shows a file in the system's file manager.
pub(crate) const REVEAL_IN_OS: &str = words("Reveal in Finder", "Reveal in File Explorer");
/// The title of `workbench.action.files.revealActiveFileInWindows`.
pub(crate) const REVEAL_ACTIVE_FILE_IN_OS: &str = words("Reveal Active File in Finder", "Reveal Active File in File Explorer");
