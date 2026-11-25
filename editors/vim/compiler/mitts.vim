" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0
"
" `:compiler mitts`, then `:make check`, `:make build`, `:make test` ...: rustc's errors
" and warnings from Mitts' output go to the quickfix list. Paths are relative to the
" workspace root, as Mitts (like Cargo) prints them, so run `:make` from there.

if exists('current_compiler')
  finish
endif
let current_compiler = 'mitts'

let s:cpo_save = &cpo
set cpo&vim

let s:mitts = get(g:, 'mitts_executable', 'mitts')
execute 'CompilerSet makeprg=' . escape(s:mitts, ' \|"')

" A diagnostic is a head line (`error[E0308]: ...`, `warning: ...`) and its location on
" the next line (` --> src/main.rs:2:9`). Summaries without a location (`error: aborting
" due to ...`, `warning: 1 warning emitted`) and Mitts' own report lines are dropped.
CompilerSet errorformat=
      \%-Gerror:\ aborting\ due\ to%.%#,
      \%-Gerror:\ could\ not\ compile%.%#,
      \%-Gwarning:\ %*\\d\ warning%.%#\ emitted,
      \%-Gwarning:\ %.%#generated\ %*\\d\ warning%.%#,
      \%-Gwarning:\ build\ failed%.%#,
      \%Eerror[E%n]:\ %m,
      \%Eerror:\ %m,
      \%Wwarning:\ %m,
      \%C\ %#-->\ %f:%l:%c,
      \%Z,
      \%-G%.%#

let &cpo = s:cpo_save
unlet s:cpo_save
