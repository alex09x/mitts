# Mitts for Sublime Text

[`Mitts.sublime-build`](Mitts.sublime-build) is a build system for
[Mitts](https://github.com/alex09x/mitts): `Ctrl+B` / `Cmd+B` runs `mitts build` from the
project folder, `Ctrl+Shift+B` / `Cmd+Shift+B` picks a variant (Check, Test, Run, Clippy,
Build --release). It is chosen automatically for Rust files in a folder with a
`Cargo.toml`. rustc's errors and warnings are links in the build panel (`F4` / `Shift+F4`
go to the next / previous one).

## Install

Copy `Mitts.sublime-build` to your `User` package: **Preferences → Browse Packages…**,
then the `User` folder. Mitts itself must be on `PATH`.

The build runs from the first folder of the window (`$folder`); open the workspace root
as that folder, since paths in rustc's messages are relative to it.

## Tests

```sh
python3 editors/sublime/test/check.py                       # the file and its regex
MITTS_SUBLIME_E2E=1 python3 editors/sublime/test/check.py   # plus a real `mitts check`
```

They check the build file's JSON and apply its `file_regex` to captured Mitts output
and to the output of a real `mitts check` on a broken project. Sublime Text itself is
not started, so its build panel is not tested. CI runs them on Linux, macOS and Windows
(`.github/workflows/editors.yml`).
