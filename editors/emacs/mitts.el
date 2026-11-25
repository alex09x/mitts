;;; mitts.el --- Run Mitts, the Rust build tool, from Emacs  -*- lexical-binding: t; -*-

;; Mitts Build for Emacs — editor integration for the Mitts Rust build tool
;; Copyright (c) 2026 Alexander Panasenko
;;
;; Contact: alex@prod.codes
;; Author: Alexander Panasenko <alex@prod.codes>
;; URL: https://github.com/alex09x/mitts
;; Version: 0.1.0
;; Package-Requires: ((emacs "27.1"))
;; Keywords: tools, rust
;; SPDX-License-Identifier: MIT OR Apache-2.0

;;; Commentary:

;; Runs Mitts (https://github.com/alex09x/mitts) with `compile' from the
;; workspace root, so rustc's errors and warnings are links in the compilation
;; buffer (`next-error').
;;
;;   M-x mitts-check, mitts-build, mitts-test, mitts-run, mitts-clippy
;;   M-x mitts-compile    any Mitts command line
;;
;; `mitts-cargo-mode' makes rust-mode and rustic run Mitts where they would run
;; Cargo.  `mitts-eglot-setup' routes rust-analyzer's on-save check and its
;; runnables through Mitts; `mitts-lsp-setup' does the check for lsp-mode.

;;; Code:

(require 'compile)
(require 'subr-x)

(defgroup mitts nil
  "Run Mitts, the Rust build tool."
  :group 'tools
  :prefix "mitts-")

(defcustom mitts-executable "mitts"
  "The Mitts command."
  :type 'string)

(defvar mitts-compile-history nil
  "Command lines given to `mitts-compile'.")

;; rustc's diagnostics: a head line, then the location on the next line.
;; Emacs 29 and later have the same pattern as `rust'.
(defconst mitts--rustc-regexp
  (concat "^\\(?:\\(?1:error\\)\\|\\(?2:warning\\)\\|\\(?3:note\\)\\)"
          "\\(?:\\[[0-9A-Z]+]\\)?:.*\n"
          " +--> \\(?4:.+\\):\\(?5:[0-9]+\\):\\(?6:[0-9]+\\)"))

(unless (assq 'rust compilation-error-regexp-alist-alist)
  (add-to-list 'compilation-error-regexp-alist-alist
               `(mitts ,mitts--rustc-regexp 4 5 6 (2 . 3) nil
                       (1 compilation-error-face) (2 compilation-warning-face)
                       (3 compilation-info-face)))
  (add-to-list 'compilation-error-regexp-alist 'mitts))

(defun mitts--workspace-manifest-p (dir)
  "Non-nil when DIR has a Cargo.toml with a [workspace] table."
  (let ((manifest (expand-file-name "Cargo.toml" dir)))
    (and (file-readable-p manifest)
         (with-temp-buffer
           (insert-file-contents manifest)
           (re-search-forward "^[ \t]*\\[workspace\\][ \t]*$" nil t)))))

(defun mitts--locate-workspace (dir)
  "The workspace root of DIR as `mitts locate-project --workspace' names it."
  (when (executable-find mitts-executable)
    (let ((default-directory dir))
      (condition-case nil
          (let ((manifest (car (process-lines
                                mitts-executable "locate-project" "--workspace"
                                "--message-format" "plain"))))
            (and manifest (file-exists-p manifest)
                 (file-name-directory manifest)))
        (error nil)))))

(defun mitts-workspace-root (&optional directory)
  "The workspace root of DIRECTORY (default: `default-directory'), or nil.
Mitts names it when it is installed, as Cargo would; otherwise the nearest
directory upwards whose Cargo.toml has a [workspace] table, else the nearest
with a Cargo.toml."
  (let* ((dir (file-name-as-directory
               (expand-file-name (or directory default-directory))))
         (nearest (locate-dominating-file dir "Cargo.toml")))
    (when nearest
      (or (mitts--locate-workspace dir)
          (let ((root (locate-dominating-file
                       nearest #'mitts--workspace-manifest-p)))
            (file-name-as-directory (expand-file-name (or root nearest))))))))

(defun mitts--command (args)
  "The shell command running Mitts with ARGS (a string)."
  (string-trim (concat (shell-quote-argument mitts-executable) " " args)))

;;;###autoload
(defun mitts-compile (args)
  "Run `mitts ARGS' with `compile' from the workspace root."
  (interactive
   (list (read-string "mitts " (car mitts-compile-history)
                      'mitts-compile-history)))
  (let ((default-directory (or (mitts-workspace-root) default-directory)))
    (compile (mitts--command args))))

;;;###autoload
(defun mitts-check ()
  "Run `mitts check'." (interactive) (mitts-compile "check"))
;;;###autoload
(defun mitts-build ()
  "Run `mitts build'." (interactive) (mitts-compile "build"))
;;;###autoload
(defun mitts-test ()
  "Run `mitts test'." (interactive) (mitts-compile "test"))
;;;###autoload
(defun mitts-run ()
  "Run `mitts run'." (interactive) (mitts-compile "run"))
;;;###autoload
(defun mitts-clippy ()
  "Run `mitts clippy'." (interactive) (mitts-compile "clippy"))

(defvar mitts-command-map
  (let ((map (make-sparse-keymap)))
    (define-key map "c" #'mitts-check)
    (define-key map "b" #'mitts-build)
    (define-key map "t" #'mitts-test)
    (define-key map "r" #'mitts-run)
    (define-key map "k" #'mitts-clippy)
    (define-key map "m" #'mitts-compile)
    map)
  "Mitts commands, for a prefix key of your choice:
\(global-set-key (kbd \"C-c m\") mitts-command-map).")
(fset 'mitts-command-map mitts-command-map)

;;; rust-mode and rustic

(defvar rust-cargo-bin)
(defvar rustic-cargo-bin)

(defconst mitts--cargo-variables '(rust-cargo-bin rustic-cargo-bin)
  "The variables naming the Cargo command in rust-mode and rustic.")

(defvar mitts--saved-cargo nil
  "Values of `mitts--cargo-variables' before `mitts-cargo-mode' was turned on.")

;;;###autoload
(define-minor-mode mitts-cargo-mode
  "Make rust-mode and rustic run Mitts where they run Cargo."
  :global t
  :group 'mitts
  (if mitts-cargo-mode
      (dolist (var mitts--cargo-variables)
        (push (cons var (if (boundp var) (default-value var) 'mitts--unbound))
              mitts--saved-cargo)
        (set-default var mitts-executable))
    (dolist (saved mitts--saved-cargo)
      (if (eq (cdr saved) 'mitts--unbound)
          (makunbound (car saved))
        (set-default (car saved) (cdr saved))))
    (setq mitts--saved-cargo nil)))

;;; rust-analyzer

(defun mitts-rust-analyzer-check-command ()
  "The command rust-analyzer runs to check the workspace through Mitts."
  (list mitts-executable "check" "--workspace" "--all-targets"
        "--message-format=json"))

(defun mitts-rust-analyzer-settings ()
  "Settings, as an Eglot plist, that run rust-analyzer's checks through Mitts.
Its runnables too."
  `(:rust-analyzer
    (:check (:overrideCommand ,(vconcat (mitts-rust-analyzer-check-command)))
     :runnables (:command ,mitts-executable))))

(defun mitts--plist-merge (base extra)
  "BASE with EXTRA's keys set, nested plists merged."
  (let ((result (copy-sequence base)))
    (while extra
      (let* ((key (pop extra))
             (value (pop extra))
             (old (plist-get result key)))
        (setq result
              (plist-put result key
                         (if (and (consp old) (keywordp (car old))
                                  (consp value) (keywordp (car value)))
                             (mitts--plist-merge old value)
                           value)))))
    result))

(defvar eglot-workspace-configuration)

;;;###autoload
(defun mitts-eglot-setup ()
  "Add the Mitts settings to the default `eglot-workspace-configuration'.
Keeps what is already there. rust-analyzer still loads the project with Cargo."
  (interactive)
  (let ((current (and (boundp 'eglot-workspace-configuration)
                      (default-value 'eglot-workspace-configuration))))
    (unless (or (null current)
                (and (consp current) (keywordp (car current))))
      (user-error "%s" (concat "eglot-workspace-configuration is not a plist; "
                               "add `mitts-rust-analyzer-settings' yourself")))
    (setq-default eglot-workspace-configuration
                  (mitts--plist-merge current (mitts-rust-analyzer-settings)))))

(defvar lsp-rust-analyzer-cargo-override-command)

;;;###autoload
(defun mitts-lsp-setup ()
  "Make lsp-mode's rust-analyzer check the workspace with Mitts.
lsp-mode has no setting for the runnables' command; they still run Cargo."
  (interactive)
  (setq-default lsp-rust-analyzer-cargo-override-command
                (vconcat (mitts-rust-analyzer-check-command))))

(provide 'mitts)
;;; mitts.el ends here
