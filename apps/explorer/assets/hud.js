// Maya2C Explorer: HUD layer (pairs with hud.css).
//
// Two canvases. A faint particle field behind every page, and on the
// dashboard a propagation core: every real block from /ws/blocks fires a
// shockwave from the core out to the four validators and the quorum's
// signatures back. Only the block data is live; the node layout is a picture.
// No dependencies; it pauses when the tab is hidden, and with reduced motion
// it draws one still frame.
(function () {
  'use strict';
  // The stylesheet is added from here, not from the page, so the HTML stays
  // the only request before first paint (apps/explorer/tests/budget_tests.rs).
  var css = document.createElement('link');
  css.rel = 'stylesheet'; css.href = '/assets/hud.css';
  document.head.appendChild(css);
  var reduce = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;
  var DPR = Math.min(window.devicePixelRatio || 1, 2);
  var COLORS = ['#22e6ff', '#ff3df2', '#ffb020', '#7dff6a', '#8b5cf6'];
  var NODES = ['SEED', 'V2', 'V3', 'V4'];

  function fit(canvas) {
    var r = canvas.getBoundingClientRect();
    canvas.width = Math.max(1, Math.round(r.width * DPR));
    canvas.height = Math.max(1, Math.round(r.height * DPR));
    var ctx = canvas.getContext('2d');
    ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
    return { ctx: ctx, w: r.width, h: r.height };
  }
  // Refit whenever the element's box changes: on resize, and once hud.css
  // arrives (it loads after this script, so the first measure is unstyled).
  function watch(el, refit) {
    if (window.ResizeObserver) new ResizeObserver(function () { refit(); }).observe(el);
    else addEventListener('resize', refit);
  }
  function hueOf(hash) {
    var n = 0, s = String(hash || '');
    for (var i = 0; i < Math.min(8, s.length); i++) n = (n * 31 + s.charCodeAt(i)) >>> 0;
    return COLORS[n % COLORS.length];
  }

  // ---------- background particle field ----------
  function field() {
    var c = document.createElement('canvas');
    c.id = 'hud-field';
    document.body.insertBefore(c, document.body.firstChild);
    var g, pts = [];
    function reset() {
      g = fit(c);
      var n = Math.round(Math.min(90, g.w * g.h / 16000));
      pts = [];
      for (var i = 0; i < n; i++) {
        pts.push({ x: Math.random() * g.w, y: Math.random() * g.h,
          vx: (Math.random() - 0.5) * 0.25, vy: (Math.random() - 0.5) * 0.25,
          c: COLORS[i % COLORS.length] });
      }
    }
    function frame() {
      var ctx = g.ctx;
      ctx.clearRect(0, 0, g.w, g.h);
      for (var i = 0; i < pts.length; i++) {
        var p = pts[i];
        p.x = (p.x + p.vx + g.w) % g.w; p.y = (p.y + p.vy + g.h) % g.h;
        for (var j = i + 1; j < pts.length; j++) {
          var q = pts[j], dx = p.x - q.x, dy = p.y - q.y, d2 = dx * dx + dy * dy;
          if (d2 < 14000) {
            ctx.strokeStyle = 'rgba(34,230,255,' + (0.12 * (1 - d2 / 14000)).toFixed(3) + ')';
            ctx.beginPath(); ctx.moveTo(p.x, p.y); ctx.lineTo(q.x, q.y); ctx.stroke();
          }
        }
        ctx.fillStyle = p.c; ctx.globalAlpha = 0.7;
        ctx.fillRect(p.x - 1, p.y - 1, 2, 2); ctx.globalAlpha = 1;
      }
    }
    reset();
    watch(c, reset);
    return frame;
  }

  // ---------- propagation core ----------
  function core() {
    var tiles = document.querySelector('.tiles');
    if (!tiles || !document.getElementById('stat-height')) return null;
    var sec = document.createElement('section');
    sec.className = 'panel hud-core';
    sec.setAttribute('aria-label', 'Live block propagation');
    sec.innerHTML =
      '<canvas></canvas>' +
      '<div class="hud-read"><span>HEIGHT <b id="hud-h">—</b></span>' +
      '<span>LAST BLOCK <b id="hud-age">—</b></span>' +
      '<span>TXS <b id="hud-tx">—</b></span></div>' +
      '<p class="hud-note">Live: every wave is a real block from this explorer’s feed. ' +
      'The four validators are drawn, not mapped.</p>';
    tiles.parentNode.insertBefore(sec, tiles);
    var canvas = sec.querySelector('canvas');
    var g = fit(canvas);
    watch(canvas, function () { g = fit(canvas); });

    var waves = [], packets = [], t0 = performance.now(), lastBlockAt = 0;
    var heightEl = document.getElementById('hud-h'), ageEl = document.getElementById('hud-age');
    var txEl = document.getElementById('hud-tx');
    var start = document.getElementById('stat-height');
    if (start) heightEl.textContent = start.textContent;

    function nodePos(i, t) {
      var a = t * 0.00012 + i * Math.PI / 2;
      var rx = Math.min(g.w * 0.42, 520), ry = Math.min(g.h * 0.36, 170);
      return { x: g.w / 2 + Math.cos(a) * rx, y: g.h / 2 + 18 + Math.sin(a) * ry, z: Math.sin(a) };
    }

    function block(b) {
      var col = hueOf(b && b.id);
      lastBlockAt = performance.now();
      if (b && b.height !== undefined) heightEl.textContent = String(b.height);
      if (b && b.tx_count !== undefined) txEl.textContent = String(b.tx_count);
      waves.push({ t: lastBlockAt, c: col });
      for (var i = 0; i < NODES.length; i++) {
        packets.push({ from: -1, to: i, t: lastBlockAt + i * 60, c: col, back: false });
      }
      var tile = start && start.closest('.tile');
      if (tile) { tile.classList.remove('hud-flash'); void tile.offsetWidth; tile.classList.add('hud-flash'); }
    }

    function ring(ctx, x, y, r, a0, a1, color, width) {
      ctx.strokeStyle = color; ctx.lineWidth = width;
      ctx.beginPath(); ctx.arc(x, y, r, a0, a1); ctx.stroke();
    }

    function frame(now) {
      var ctx = g.ctx, t = now - t0, cx = g.w / 2, cy = g.h / 2 + 18;
      ctx.clearRect(0, 0, g.w, g.h);

      // Rotating dot sphere.
      var R = Math.min(g.w * 0.32, g.h * 0.3), spin = t * 0.0003;
      for (var la = -60; la <= 60; la += 20) {
        for (var lo = 0; lo < 360; lo += 15) {
          var phi = la * Math.PI / 180, th = lo * Math.PI / 180 + spin;
          var x = Math.cos(phi) * Math.cos(th), z = Math.cos(phi) * Math.sin(th), y = Math.sin(phi);
          var s = 1 / (1.8 - z * 0.6);
          ctx.fillStyle = 'rgba(34,230,255,' + (0.15 + 0.45 * (z + 1) / 2).toFixed(3) + ')';
          ctx.fillRect(cx + x * R * s - 1, cy + y * R * s * 0.9 - 1, 2, 2);
        }
      }

      // Radar sweep and a ticked bezel, turning slowly.
      var sweep = t * 0.0009;
      if (ctx.createConicGradient) {
        var cg = ctx.createConicGradient(sweep, cx, cy);
        cg.addColorStop(0, 'rgba(34,230,255,0.28)');
        cg.addColorStop(0.08, 'rgba(34,230,255,0)');
        cg.addColorStop(1, 'rgba(34,230,255,0)');
        ctx.fillStyle = cg; ctx.beginPath(); ctx.arc(cx, cy, R * 1.05, 0, Math.PI * 2); ctx.fill();
      }
      for (var tk = 0; tk < 120; tk++) {
        var ta = tk / 120 * Math.PI * 2 - t * 0.00015, long = tk % 10 === 0;
        var r1 = R * 1.08, r2 = R * (long ? 1.16 : 1.12);
        ctx.strokeStyle = long ? 'rgba(255,176,32,0.85)' : 'rgba(34,230,255,0.35)';
        ctx.lineWidth = long ? 2 : 1;
        ctx.beginPath();
        ctx.moveTo(cx + Math.cos(ta) * r1, cy + Math.sin(ta) * r1);
        ctx.lineTo(cx + Math.cos(ta) * r2, cy + Math.sin(ta) * r2);
        ctx.stroke();
      }
      // Segmented outer ring: eight dashes, counter-rotating.
      ctx.setLineDash([R * 0.32, R * 0.12]);
      ring(ctx, cx, cy, R * 1.22, -t * 0.0004, -t * 0.0004 + Math.PI * 2, 'rgba(139,92,246,0.7)', 3);
      ctx.setLineDash([]);

      // Core: three counter-rotating arcs and a breathing centre.
      ring(ctx, cx, cy, R * 0.55, t * 0.0016, t * 0.0016 + 4.2, 'rgba(34,230,255,0.9)', 2);
      ring(ctx, cx, cy, R * 0.68, -t * 0.0011, -t * 0.0011 + 2.6, 'rgba(255,61,242,0.85)', 2);
      ring(ctx, cx, cy, R * 0.80, t * 0.0007, t * 0.0007 + 1.4, 'rgba(255,176,32,0.8)', 3);
      var breathe = 0.6 + 0.4 * Math.sin(t * 0.003);
      var grd = ctx.createRadialGradient(cx, cy, 0, cx, cy, R * 0.45);
      grd.addColorStop(0, 'rgba(230,250,255,' + (0.85 * breathe).toFixed(3) + ')');
      grd.addColorStop(0.35, 'rgba(34,230,255,0.35)');
      grd.addColorStop(1, 'rgba(34,230,255,0)');
      ctx.fillStyle = grd; ctx.beginPath(); ctx.arc(cx, cy, R * 0.45, 0, Math.PI * 2); ctx.fill();

      // Shockwaves.
      waves = waves.filter(function (w) {
        var k = (now - w.t) / 1600;
        if (k > 1) return false;
        ctx.globalAlpha = 1 - k;
        ring(ctx, cx, cy, R * 0.4 + k * Math.max(g.w, g.h) * 0.55, 0, Math.PI * 2, w.c, 3 * (1 - k) + 0.5);
        ctx.globalAlpha = 1;
        return true;
      });

      // Validator nodes and their links to the core.
      var pos = [];
      for (var i = 0; i < NODES.length; i++) {
        var p = nodePos(i, t); pos.push(p);
        ctx.strokeStyle = 'rgba(34,230,255,0.18)'; ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(cx, cy); ctx.lineTo(p.x, p.y); ctx.stroke();
      }
      for (i = 0; i < pos.length; i++) {
        var q = pos[i], r = 10 + q.z * 3;
        ring(ctx, q.x, q.y, r + 6, t * 0.004 + i, t * 0.004 + i + 4, COLORS[i], 1.5);
        ctx.fillStyle = COLORS[i]; ctx.shadowColor = COLORS[i]; ctx.shadowBlur = 16;
        ctx.beginPath(); ctx.arc(q.x, q.y, r, 0, Math.PI * 2); ctx.fill(); ctx.shadowBlur = 0;
        ctx.fillStyle = '#e6f6ff'; ctx.font = '600 11px ui-monospace, Consolas, monospace';
        ctx.fillText(NODES[i], q.x + r + 10, q.y + 4);
      }

      // Packets: the block out to each validator, then a signature back.
      packets = packets.filter(function (k) {
        var u = (now - k.t) / 700;
        if (u < 0) return true;
        if (u >= 1) {
          if (!k.back) packets.push({ from: k.to, to: -1, t: now + 120, c: '#7dff6a', back: true });
          return false;
        }
        var a = k.from < 0 ? { x: cx, y: cy } : pos[k.from];
        var b = k.to < 0 ? { x: cx, y: cy } : pos[k.to];
        var e = u * u * (3 - 2 * u), x2 = a.x + (b.x - a.x) * e, y2 = a.y + (b.y - a.y) * e;
        ctx.strokeStyle = k.c; ctx.lineWidth = 2; ctx.shadowColor = k.c; ctx.shadowBlur = 12;
        var tail = Math.max(0, e - 0.18);
        ctx.beginPath();
        ctx.moveTo(a.x + (b.x - a.x) * tail, a.y + (b.y - a.y) * tail);
        ctx.lineTo(x2, y2); ctx.stroke();
        ctx.fillStyle = '#fff'; ctx.beginPath(); ctx.arc(x2, y2, 2.6, 0, Math.PI * 2); ctx.fill();
        ctx.shadowBlur = 0;
        return true;
      });

      if (lastBlockAt) ageEl.textContent = ((now - lastBlockAt) / 1000).toFixed(1) + ' S';
    }

    // Live blocks: the same feed the page already uses, on its own socket.
    (function connect() {
      var proto = location.protocol === 'https:' ? 'wss:' : 'ws:', ws;
      try { ws = new WebSocket(proto + '//' + location.host + '/ws/blocks'); } catch (e) { return; }
      ws.onmessage = function (ev) { try { block(JSON.parse(ev.data)); } catch (e) {} };
      ws.onclose = function () { setTimeout(connect, 3000); };
    })();
    block(null);
    return frame;
  }

  function boot() {
    var frames = [field(), core()].filter(Boolean);
    var draw = function (now) { frames.forEach(function (f) { f(now); }); };
    if (reduce) { draw(performance.now()); return; }
    (function loop(now) {
      if (!document.hidden) draw(now);
      requestAnimationFrame(loop);
    })(performance.now());
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
  else boot();
})();
