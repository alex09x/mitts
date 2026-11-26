# Changelog

## 0.1.1

- **Install Mitts** works on Windows: it runs the PowerShell installer (from PowerShell,
  pwsh and cmd terminals). 0.1.0 said Mitts does not support Windows, which it does since
  Mitts 0.5.0.
- Finding Mitts checks Windows paths with Windows rules (and POSIX paths with POSIX ones).
- Tested as an installed VSIX in VS Code on Linux, macOS and Windows before it is published.

## 0.1.0

- Finds or installs Mitts and shows its version in the status bar.
- Tasks for build, check, clippy, test, bench, run, doc and clean, with the `$mitts` problem matcher.
- Opt-in commands that route rust-analyzer checks and runnables through Mitts, and back.
