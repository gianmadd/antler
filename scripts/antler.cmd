@echo off
rem antler [--depth N] [--hide-clean] [FOLDER]: opens FOLDER (default: the current one) in Antler, like `code .`.
rem The Windows zip has it in bin\, next to ..\antler.exe (like VS Code's bin\code.cmd): add that bin folder to PATH.
setlocal
rem before any shift, which moves %0 too
set "exe=%~dp0..\antler.exe"
set "folder="
:args
if "%~1"=="" goto run
set "arg=%~1"
if "%arg%"=="--depth" (
    shift
    shift
    goto args
)
if "%arg:~0,1%"=="-" (
    shift
    goto args
)
set "folder=1"
shift
goto args
:run
rem start: return at once, the app is a window (a batch file would wait for it). Without a folder, `.`: the app
rem resolves it against the current folder, which it inherits (the current folder itself ends in \ at a drive root,
rem and a quoted C:\ would end in an escaped quote).
if defined folder (
    start "" "%exe%" %*
) else (
    start "" "%exe%" %* .
)
