" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0

let s:dir = expand('<sfile>:p:h')

" The quickfix entries `:compiler mitts` reads from `lines`: valid ones only.
function! s:Problems(lines) abort
  compiler! mitts
  let l:items = getqflist({'lines': a:lines, 'efm': &errorformat}).items
  let l:found = []
  for l:item in filter(l:items, 'v:val.valid')
    call add(l:found, {
          \ 'type': l:item.type,
          \ 'nr': l:item.nr,
          \ 'text': l:item.text,
          \ 'file': substitute(bufname(l:item.bufnr), '\\', '/', 'g'),
          \ 'lnum': l:item.lnum,
          \ 'col': l:item.col,
          \ })
  endfor
  return l:found
endfunction

" Captured Mitts output: a warning, a coded and an uncoded error, with Mitts' own command
" line, summaries and report around them.
function! Test_errors_and_warnings_from_mitts_output() abort
  let l:found = s:Problems(readfile(s:dir . '/output/check.txt'))
  call assert_equal([
        \ {'type': 'W', 'nr': -1, 'text': 'unused variable: `unused`',
        \  'file': 'crates/a/src/main.rs', 'lnum': 2, 'col': 9},
        \ {'type': 'E', 'nr': 308, 'text': 'mismatched types',
        \  'file': 'crates/a/src/lib.rs', 'lnum': 1, 'col': 27},
        \ {'type': 'E', 'nr': -1, 'text': 'literal out of range for `u8`',
        \  'file': 'crates/a/src/lib.rs', 'lnum': 3, 'col': 20},
        \ ], map(l:found, 'extend(v:val, {"nr": v:val.nr == 0 ? -1 : v:val.nr})'))
endfunction

" Summaries carry no location and are no problems of their own.
function! Test_summaries_are_not_problems() abort
  call assert_equal([], s:Problems([
        \ 'error: aborting due to 2 previous errors',
        \ 'warning: 1 warning emitted',
        \ 'warning: `a` (lib) generated 3 warnings',
        \ 'error: could not compile `a` (lib) due to 1 previous error',
        \ ]))
endfunction

function! Test_makeprg_is_mitts_or_the_configured_executable() abort
  compiler! mitts
  call assert_equal('mitts', &makeprg)
  let g:mitts_executable = '/opt/mitts dir/mitts'
  unlet! g:current_compiler
  compiler! mitts
  call assert_equal('/opt/mitts dir/mitts', &makeprg)
  unlet g:mitts_executable
  unlet! g:current_compiler
  compiler! mitts
endfunction

function! Test_mitts_command_completes_subcommands() abort
  call assert_equal(['check', 'clean', 'clippy'], mitts#complete('c', 'Mitts c', 7))
  call assert_equal([], mitts#complete('', 'Mitts build ', 12))
  call assert_equal(2, exists(':Mitts'))
endfunction

" A Rust buffer gets `:compiler mitts` when Mitts is on PATH (here a stand-in script).
function! Test_rust_buffers_get_the_mitts_compiler() abort
  let l:bin = tempname()
  call mkdir(l:bin, 'p')
  let l:exe = l:bin . (has('win32') ? '/mitts.bat' : '/mitts')
  call writefile(has('win32') ? ['@exit /b 0'] : ['#!/bin/sh', 'exit 0'], l:exe)
  if !has('win32')
    call setfperm(l:exe, 'rwxr-xr-x')
  endif
  let l:path = $PATH
  let $PATH = l:bin . (has('win32') ? ';' : ':') . $PATH
  try
    new
    setfiletype rust
    call assert_equal('mitts', &l:makeprg)
    call assert_equal('mitts', get(b:, 'current_compiler', ''))
    let g:mitts_set_compiler = 0
    new
    setfiletype rust
    call assert_notequal('mitts', get(b:, 'current_compiler', ''))
  finally
    unlet! g:mitts_set_compiler
    let $PATH = l:path
  endtry
endfunction

" `:Mitts` leaves the buffer's own compiler settings as they were.
function! Test_mitts_command_restores_the_buffer_compiler() abort
  new
  setlocal makeprg=make errorformat=%f:%l:%m
  let g:mitts_executable = has('win32') ? 'cmd /c exit 0 &&' : 'true'
  try
    silent Mitts! check
  finally
    unlet g:mitts_executable
  endtry
  call assert_equal('make', &l:makeprg)
  call assert_equal('%f:%l:%m', &l:errorformat)
endfunction
