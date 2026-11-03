# Antler

A minimal, fast IDE for macOS: explorer, editor, git and remote over SSH. Nested repositories under a folder all show up in one panel.

## Workspace

**Root**:
The folder open in a window; everything Antler shows lives under it.
_Avoid_: Workspace, project

**Window**:
One of the windows of the single Antler process. Each has its own Root (or none), tabs, editor groups and git queue; settings and keybindings are shared. A folder has at most one Window.

**Remote host**:
A machine reached over SSH whose folders Antler can open; Linux, macOS or Windows.
_Avoid_: Server, remote machine

**Remote window**:
A Window whose Root lives on a Remote host; it shows the host's name and works like a local Window.
_Avoid_: SSH session, remote workspace

**Forwarded port**:
A port of the Remote host made reachable at `localhost` on the Mac while the Remote window is connected.
_Avoid_: Tunnel

**Nested repository**:
A git repository found under the Root at any depth, even inside another repository or inside a folder that repository ignores. Each submodule counts as a Nested repository of its own.
_Avoid_: Sub-repo, nested repo, worktree

**Scan depth**:
The maximum number of folder levels under the Root searched for Nested repositories; a repo directly inside the Root is at depth 1.
_Avoid_: file_scan_depth

**Linked worktree**:
A Nested repository created with `git worktree add` from another repository, whose history it shares; it appears as a repo of its own but belongs to a main repo.
_Avoid_: Worktree (alone), copy, clone

**Repository section (Section)**:
The collapsible block of the Git view dedicated to one Nested repository, with its own commit message, branch and change groups.
_Avoid_: Tab, repo tab

## Interface

**Activity bar**:
The vertical column of icons on the left used to choose which view the side panel shows.
_Avoid_: Sidebar

**Explorer view**:
The side panel with the tree of files and folders under the Root.
_Avoid_: File tree, project panel

**Search view**:
The side panel for searching text across all files under the Root.
_Avoid_: Search panel, find in files

**Git view**:
The side panel with the Repository sections of all Nested repositories.
_Avoid_: Source Control, SCM, git panel

**Editor group (Group)**:
One tab strip with its tabs and the content of its active tab; the editor area holds one or more, side by side or one over the other. A file open in two Groups has one text.
_Avoid_: Pane, split, column

**Untitled file**:
A file opened with no path yet ("Untitled-1"); it gets one on its first save.
_Avoid_: New file, scratch buffer

**Command palette**:
The list of every Antler command, opened from Quick Open with `>`.
_Avoid_: Command menu, actions list

## Changes

**Hunk**:
A contiguous group of changed lines in a diff, the smallest unit of stage, unstage and revert alongside selected lines.
_Avoid_: Chunk

**Partial staging**:
Staging only some Hunks or lines of a file.
_Avoid_: Partial add, patch add

## Synchronization

**Sync**:
Pull followed by push on a single Nested repository. There is no Sync that acts on several repositories at once.
_Avoid_: Sync all, update

**Auto fetch**:
The periodic background fetch of all Nested repositories, which updates the incoming and outgoing commit counters without touching the working tree.
_Avoid_: Auto-sync, polling
