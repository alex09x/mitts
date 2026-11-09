/*
 * Mitts Build — machine and measure tabs for The Hundred page and the landing scoreboard
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
(function () {
  document.documentElement.classList.add("js");
  // Tab groups: <div class="tabs" data-key="pl"> buttons switch every .panel that carries data-pl.
  // Without this script every panel is simply shown; the pages are complete static HTML.
  var state = {};
  var groups = document.querySelectorAll(".tabs");
  function show() {
    document.querySelectorAll(".panel").forEach(function (p) {
      var ok = true;
      Object.keys(state).forEach(function (k) {
        var v = p.getAttribute("data-" + k);
        if (v !== null && v !== state[k]) ok = false;
      });
      p.hidden = !ok;
    });
  }
  groups.forEach(function (g) {
    var key = g.getAttribute("data-key");
    var first = g.querySelector("button");
    if (first) state[key] = first.getAttribute("data-v");
    g.addEventListener("click", function (e) {
      var b = e.target.closest("button");
      if (!b) return;
      g.querySelectorAll("button").forEach(function (x) { x.classList.toggle("on", x === b); });
      state[key] = b.getAttribute("data-v");
      show();
    });
  });
  show();
})();
