" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0
"
" Neovim only: the Lua module routes rust-analyzer's check and runnables through Mitts.

function! Test_lua_rust_analyzer_settings() abort
  if !has('nvim')
    return
  endif
  let l:s = luaeval('require("mitts").rust_analyzer_settings()')
  call assert_equal(['mitts', 'check', '--workspace', '--all-targets', '--message-format=json'],
        \ l:s['rust-analyzer'].check.overrideCommand)
  call assert_equal('mitts', l:s['rust-analyzer'].runnables.command)
  call assert_equal('/x/mitts',
        \ luaeval('require("mitts").rust_analyzer_settings("/x/mitts")["rust-analyzer"].runnables.command'))
endfunction

" setup() merges into vim.lsp.config's rust_analyzer, keeping what is already there.
function! Test_lua_setup_merges_into_vim_lsp_config() abort
  if !has('nvim-0.11')
    return
  endif
  lua vim.lsp.config("rust_analyzer", { cmd = { "rust-analyzer" }, settings = { ["rust-analyzer"] = { cargo = { features = "all" } } } })
  lua require("mitts").setup()
  let l:c = luaeval('vim.lsp.config.rust_analyzer')
  call assert_equal(['rust-analyzer'], l:c.cmd)
  call assert_equal('all', l:c.settings['rust-analyzer'].cargo.features)
  call assert_equal('mitts', l:c.settings['rust-analyzer'].check.overrideCommand[0])
endfunction
