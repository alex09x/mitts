-- Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
-- Copyright (c) 2026 Alexander Panasenko
--
-- Contact: alex@prod.codes
-- Author: https://prod.codes/about/
-- Project: https://github.com/alex09x/mitts
-- SPDX-License-Identifier: MIT OR Apache-2.0
--
-- Neovim: routes rust-analyzer's on-save check and its runnables through Mitts, as the VS
-- Code extension does. rust-analyzer still loads the project with `cargo metadata`.
--
--   require("mitts").setup()                       -- Neovim 0.11+, vim.lsp.config
--   settings = require("mitts").rust_analyzer_settings()   -- for lspconfig or rustaceanvim

local M = {}

--- The executable: `vim.g.mitts_executable`, else `mitts`.
function M.executable()
  return vim.g.mitts_executable or "mitts"
end

--- rust-analyzer settings (the table under `settings`) that run checks and runnables
--- through Mitts.
function M.rust_analyzer_settings(executable)
  local exe = executable or M.executable()
  return {
    ["rust-analyzer"] = {
      check = {
        overrideCommand = { exe, "check", "--workspace", "--all-targets", "--message-format=json" },
      },
      runnables = { command = exe },
    },
  }
end

--- Adds the settings to the `rust_analyzer` configuration of `vim.lsp.config` (merged with
--- what is already there). `opts.rust_analyzer = false` leaves rust-analyzer alone.
function M.setup(opts)
  opts = opts or {}
  if opts.executable then
    vim.g.mitts_executable = opts.executable
  end
  if opts.rust_analyzer ~= false and vim.lsp and vim.lsp.config then
    vim.lsp.config("rust_analyzer", { settings = M.rust_analyzer_settings() })
  end
end

return M
