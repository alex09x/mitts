" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0
"
" :Mitts {args}   runs `mitts {args}` through :make, so errors land in the quickfix list
"                 (`:Mitts check`, `:Mitts build --release`, `:Mitts test`).
" :Mitts! {args}  the same without jumping to the first error.

if exists('g:loaded_mitts')
  finish
endif
let g:loaded_mitts = 1

command! -nargs=* -bang -complete=customlist,mitts#complete Mitts call mitts#run(<q-args>, <bang>0)
