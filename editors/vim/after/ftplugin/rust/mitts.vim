" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0
"
" Rust buffers get `:compiler mitts` (after rust.vim's own `:compiler cargo`), so a plain
" `:make check` runs Mitts. `let g:mitts_set_compiler = 0` keeps the buffer's compiler.

if !get(g:, 'mitts_set_compiler', 1) || !executable(get(g:, 'mitts_executable', 'mitts'))
  finish
endif

" `:compiler` skips a script once `current_compiler` is set; another compiler set it.
unlet! b:current_compiler
compiler mitts
let s:undo = 'setlocal makeprg< errorformat< | unlet! b:current_compiler'
let b:undo_ftplugin = empty(get(b:, 'undo_ftplugin', ''))
      \ ? s:undo : b:undo_ftplugin . ' | ' . s:undo
