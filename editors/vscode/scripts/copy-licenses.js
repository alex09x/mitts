/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
// The package carries the repository's own license texts, copied at packaging time.
const fs = require("fs");
const path = require("path");
for (const name of ["LICENSE-MIT", "LICENSE-APACHE"]) {
  fs.copyFileSync(path.join(__dirname, "..", "..", "..", name), path.join(__dirname, "..", name));
}
