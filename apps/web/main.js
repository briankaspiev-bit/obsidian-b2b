// Obsidian B2B landing page: booth animation and waitlist form.
(function () {
  'use strict';

  var reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ---------- booth readout ----------
  var canvas = document.getElementById('wave');
  var ctx = canvas && canvas.getContext('2d');
  var deckA = document.getElementById('deck-a');
  var deckB = document.getElementById('deck-b');
  var roleA = document.getElementById('role-a');
  var roleB = document.getElementById('role-b');
  var msEl = document.getElementById('ms');
  var timerEl = document.getElementById('timer');
  var watchingEl = document.getElementById('watching');

  var onAir = 'a';
  var seconds = 41 * 60 + 7;
  var watching = 1842;
  var t0 = performance.now();

  function size() {
    if (!canvas) return;
    var dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = canvas.clientWidth * dpr;
    canvas.height = canvas.clientHeight * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  // Two overlapping waveforms; the one on air is bright, the cueing one is faint.
  function draw(now) {
    if (!ctx) return;
    var w = canvas.clientWidth;
    var h = canvas.clientHeight;
    var t = (now - t0) / 1000;
    ctx.clearRect(0, 0, w, h);
    var bars = Math.floor(w / 5);
    for (var i = 0; i < bars; i++) {
      var x = i * 5;
      var beat = Math.pow(Math.max(0, Math.sin((i / bars) * Math.PI * 16 - t * 4.13)), 6);
      var a = 0.18 + 0.55 * beat + 0.2 * Math.abs(Math.sin(i * 0.37 + t * 1.3));
      var b = 0.12 + 0.35 * Math.abs(Math.sin(i * 0.21 - t * 0.9)) * (0.6 + 0.4 * beat);
      var air = onAir === 'a' ? a : b;
      var cue = onAir === 'a' ? b : a;
      ctx.fillStyle = 'rgba(108,196,255,0.35)';
      ctx.fillRect(x, h / 2 - (cue * h) / 2.4, 2, (cue * h) / 1.2);
      ctx.fillStyle = 'rgba(255,106,61,0.9)';
      ctx.fillRect(x + 2, h / 2 - (air * h) / 2.2, 2, (air * h) / 1.1);
    }
    if (!reduceMotion) requestAnimationFrame(draw);
  }

  function handoff() {
    onAir = onAir === 'a' ? 'b' : 'a';
    deckA.dataset.role = onAir === 'a' ? 'air' : 'cue';
    deckB.dataset.role = onAir === 'b' ? 'air' : 'cue';
    roleA.textContent = onAir === 'a' ? 'ON AIR' : 'CUEING NEXT';
    roleB.textContent = onAir === 'b' ? 'ON AIR' : 'CUEING NEXT';
  }

  function pad(n) { return n < 10 ? '0' + n : String(n); }

  function tick() {
    seconds++;
    timerEl.textContent = pad(Math.floor(seconds / 3600)) + ':' + pad(Math.floor(seconds / 60) % 60) + ':' + pad(seconds % 60);
    msEl.textContent = 36 + Math.round(Math.random() * 5) + ' ms';
    watching += Math.round(Math.random() * 6) - 2;
    watchingEl.textContent = watching.toLocaleString('en-US');
    if (seconds % 8 === 0) handoff();
  }

  if (canvas) {
    size();
    window.addEventListener('resize', size);
    requestAnimationFrame(draw);
    if (!reduceMotion) setInterval(tick, 1000);
  }

  // ---------- waitlist ----------
  // Placeholder: no backend yet. Entries stay in this browser only.
  var form = document.getElementById('waitlist-form');
  var done = document.getElementById('waitlist-done');
  var email = document.getElementById('wl-email');
  var error = document.getElementById('wl-error');
  var doneTitle = document.getElementById('done-title');

  form.addEventListener('submit', function (e) {
    e.preventDefault();
    var value = email.value.trim();
    var valid = /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value);
    error.hidden = valid;
    email.setAttribute('aria-invalid', valid ? 'false' : 'true');
    if (!valid) { email.focus(); return; }

    var data = Object.fromEntries(new FormData(form).entries());
    try {
      var list = JSON.parse(localStorage.getItem('obsidian-waitlist') || '[]');
      list.push(Object.assign(data, { at: new Date().toISOString() }));
      localStorage.setItem('obsidian-waitlist', JSON.stringify(list));
    } catch (err) { /* storage blocked; the confirmation still shows */ }

    var name = (data.name || '').trim();
    doneTitle.textContent = name ? 'See you in the booth, ' + name + '.' : 'See you in the booth.';
    form.hidden = true;
    done.hidden = false;
  });
})();
