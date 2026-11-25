" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0
"
" With MITTS_VIM_E2E=1 and Mitts on PATH: `:Mitts! check` on a broken project puts its
" type error into the quickfix list.

let s:fixture = expand('<sfile>:p:h') . '/fixtures/broken'

function! Test_e2e_mitts_check_fills_the_quickfix_list() abort
  if $MITTS_VIM_E2E !=# '1'
    return
  endif
  call assert_true(executable('mitts'), 'mitts on PATH')
  let l:cwd = getcwd()
  execute 'cd ' . fnameescape(s:fixture)
  try
    silent Mitts! check
  finally
    execute 'cd ' . fnameescape(l:cwd)
  endtry
  let l:errors = filter(getqflist(), 'v:val.valid && v:val.type ==# "E"')
  call assert_equal(1, len(l:errors), string(getqflist()))
  if len(l:errors) == 1
    call assert_equal('src/main.rs', substitute(fnamemodify(bufname(l:errors[0].bufnr), ':.'), '\\', '/', 'g')[-11:])
    call assert_equal([2, 18, 308], [l:errors[0].lnum, l:errors[0].col, l:errors[0].nr])
    call assert_equal('mismatched types', l:errors[0].text)
  endif
endfunction
