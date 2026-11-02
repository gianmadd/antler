# Settings and keybindings use VS Code's names and formats

Keys in `~/.config/antler/settings.json` use VS Code's name when the setting exists there (`editor.fontSize`, `files.autoSave`, `git.enableSmartCommit`, `git.autofetch`…) and the `antler.` prefix otherwise (e.g. the scan depth). `~/.config/antler/keybindings.json` uses VS Code's format (`[{ "key", "command", "when" }]`, `-command` removes a binding), but `when` accepts only Antler's own contexts (editor, explorer, git, search, commit…), not VS Code's full expression language. Decided on 2026-11-02 so that someone coming from VS Code can copy their settings and keybindings over.

The keys used before this decision (`git_enable_smart_commit`, `auto_save`…) are read once at startup and rewritten with the new names.

## Considered options

- Keep Antler's own snake_case names: no migration needed, but nothing could be copied over from VS Code.
- Accept VS Code's full `when` expressions: a small expression language to write and maintain for no current need. Rules with an unknown context are ignored and a warning is shown.
