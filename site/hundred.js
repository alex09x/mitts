/*
 * Mitts Build — benchmark project table (sorting, filtering, machine picker)
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

  var node = document.getElementById("bench-data");
  if (!node) return;
  var data = JSON.parse(node.textContent);
  var sel = document.getElementById("plat");
  var flt = document.getElementById("flt");
  var body = document.querySelector("#ptable tbody");
  var head = document.querySelectorAll("#ptable th");
  var key = "name", dir = 1;

  data.forEach(function (p, i) {
    var o = document.createElement("option");
    o.value = i;
    o.textContent = p.label;
    sel.appendChild(o);
  });

  function num(v, d) {
    return v == null ? "–" : v.toFixed(d);
  }

  function rows() {
    var q = flt.value.trim().toLowerCase();
    return data[sel.value].rows
      .filter(function (r) { return !q || r.n.toLowerCase().indexOf(q) >= 0; })
      .map(function (r) {
        return {
          name: r.n, units: r.u, swarm: r.sw, swarm_sc: r.ss, swarm_x: r.sw && r.ss ? r.ss / r.sw : null,
          cold: r.cm, cold_c: r.cc, noop: r.no, status: r.s
        };
      })
      .sort(function (a, b) {
        var x = a[key], y = b[key];
        if (x == null) return 1;
        if (y == null) return -1;
        return (x < y ? -1 : x > y ? 1 : 0) * dir;
      });
  }

  function draw() {
    body.innerHTML = "";
    rows().forEach(function (r) {
      var tr = document.createElement("tr");
      r.status = r.status === "OK" ? "steady" : "wide spread";
      [r.name, r.units == null ? "–" : r.units, num(r.swarm, 2), num(r.swarm_sc, 2),
       r.swarm_x ? r.swarm_x.toFixed(2) + "×" : "–", num(r.cold, 2), num(r.cold_c, 2), num(r.noop, 1), r.status]
        .forEach(function (t, i) {
          var td = document.createElement("td");
          td.textContent = t;
          if (i === 4 && r.swarm_x) td.className = r.swarm_x >= 1 ? "good" : "bad";
          tr.appendChild(td);
        });
      body.appendChild(tr);
    });
  }

  head.forEach(function (th) {
    th.addEventListener("click", function () {
      var k = th.getAttribute("data-k");
      dir = k === key ? -dir : (k === "name" ? 1 : -1);
      key = k;
      draw();
    });
  });
  sel.addEventListener("change", draw);
  flt.addEventListener("input", draw);
  draw();
})();
