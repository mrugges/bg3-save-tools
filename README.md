# BG3 Save Tools

Back up, restore and revive Baldur's Gate 3 **Honour Mode** runs.

When an Honour Mode party wipes and you choose to continue in Custom Mode, the save itself isn't changed. The game instead adds the run's session ID to `DisabledSingleSaveSessions` in your player profile (`profile8.lsf`). Removing it brings Honour Mode back. This tool does that safely, and keeps backups of your Honour runs so a wipe doesn't have to be final.

## Requirements
- Windows, with BG3 installed (Steam version tested)
- [LSLib](https://github.com/Norbyte/lslib/releases)'s `Divine.exe`, which reads and writes BG3's file formats. The app can download it for you.

## The app
Run `bg3-save-tools-gui.exe`. On first launch it offers to download LSLib (or lets you point it at an existing `Divine.exe`). Then it lists your Honour runs with party, act and playtime, and lets you:
- **Back up now**: copy the run's save to `Documents\BG3 Save Tools\backups`
- **Restore**: put a backup back as the run's save
- **Revive Honour Mode**: undo the switch to Custom Mode after a party wipe

Close the game before reviving or restoring; the app waits until it's closed.

## Command line
```
bg3-save-tools list [--all]                    Honour runs and their status
bg3-save-tools backup <id>... | --all          back up runs' saves
bg3-save-tools backups [<id>]                  list backups
bg3-save-tools restore <id> [--backup <name>]  restore the newest (or a named) backup
bg3-save-tools revive <id>...                  revive Honour Mode
```
IDs are the GUID at the start of a save folder name (`Savegames\Story\<id>__HonourMode`); any unique prefix works. `Divine.exe` is found via `--divine`, the `DIVINE_EXE` environment variable, the app's saved setting, or next to the executable.

## Safety
- Writes are refused while BG3 is running.
- Before reviving, both the local profile and Steam Cloud copies are backed up to `Documents\BG3 Save Tools\profile-backups`.
- Before restoring, the save being replaced is backed up (`…_before-restore`).
- Changes are written to the local files and Steam's local cloud copies (`Steam\userdata\<account>\1086940\remote`). If Steam reports a cloud sync conflict on the next launch, keep the **local** files.

## Limitations
- Custom characters show as "(custom)": their names are stored in save data nobody has decoded yet. Companions and the party leader are named.
- Playtime is read from the save's `TimeStamp` field.

## Building
```
cargo build --release
```
On a machine where Git for Windows' `link.exe` shadows MSVC's, use `build.bat build --release` instead.

## Credit
[Norbyte](https://github.com/Norbyte)'s [LSLib](https://github.com/Norbyte/lslib) does all the BG3 file format work. Inspired by [bg3d](https://github.com/clemarescx/bg3d).
