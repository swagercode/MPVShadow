function setText(id, value) {
  var el = document.getElementById(id);
  if (el) el.textContent = value ?? '';
}

function trimLeadingParenGroups(s) {
  if (!s) return s;
  var prev;
  var out = String(s);
  var re = /^(?:[\(（][^\)）]*[\)）]\s*)+/;
  do {
    prev = out;
    out = out.replace(re, '');
  } while (out !== prev);
  return out;
}

function computeMedianFromSeries(seriesHz) {
  if (!seriesHz || !seriesHz.length) return null;
  var vals = seriesHz.filter(function(x){ return typeof x === 'number' && x > 0; }).slice().sort(function(a,b){return a-b;});
  if (!vals.length) return null;
  var mid = Math.floor(vals.length / 2);
  if (vals.length % 2 === 1) return vals[mid];
  return 0.5 * (vals[mid - 1] + vals[mid]);
}

function drawPitch(seriesHz, medianHz) {
  var cv = document.getElementById('pitch-canvas');
  if (!cv) return;
  var ctx = cv.getContext('2d');
  ctx.clearRect(0, 0, cv.width, cv.height);
  if (!seriesHz || !seriesHz.length) return;
  if (!(medianHz > 0)) medianHz = computeMedianFromSeries(seriesHz) || 0;
  if (!(medianHz > 0)) return;

  // Convert Hz to semitone offsets relative to median (no clamping; we'll autoscale)
  var st = new Array(seriesHz.length);
  for (var i = 0; i < seriesHz.length; i++) {
    var f = seriesHz[i];
    if (f > 0 && medianHz > 0) {
      st[i] = 12 * Math.log2(f / medianHz);
    } else {
      st[i] = null; // unvoiced
    }
  }

  // Optional tiny smoothing (3-point moving average) only on voiced points
  var st2 = st.slice();
  for (var i = 1; i + 1 < st.length; i++) {
    if (st[i - 1] != null && st[i] != null && st[i + 1] != null) {
      st2[i] = (st[i - 1] + st[i] + st[i + 1]) / 3;
    }
  }

  var w = cv.width, h = cv.height;
  // Determine dynamic Y range from voiced points with a small padding
  var voicedVals = st2.filter(function(v){ return v != null; });
  if (!voicedVals.length) return;
  var minSt = Math.min.apply(null, voicedVals);
  var maxSt = Math.max.apply(null, voicedVals);
  var range = maxSt - minSt;
  if (!(range > 0)) { // nearly flat; expand a bit for visibility
    minSt -= 0.5; maxSt += 0.5; range = maxSt - minSt;
  }
  var pad = range * 0.1;
  minSt -= pad; maxSt += pad; range = maxSt - minSt;

  ctx.lineWidth = 2.0;
  ctx.strokeStyle = '#FFF';
  ctx.beginPath();
  var started = false;
  for (var i = 0; i < st2.length; i++) {
    var yVal = st2[i];
    var x = (i / Math.max(1, st2.length - 1)) * (w - 1);
    if (yVal == null) {
      started = false; // break the stroke on unvoiced gaps
      continue;
    }
    var norm = (yVal - minSt) / (range || 1);
    var y = (1 - norm) * (h - 1);
    if (!started) { ctx.moveTo(x, y); started = true; }
    else { ctx.lineTo(x, y); }
  }
  ctx.stroke();
}

window.addEventListener('analysis', function (e) {
  var d = e.detail || {};
  if ('text' in d) {
    var tv = (typeof d.text === 'string') ? trimLeadingParenGroups(d.text) : (d.text ?? '');
    if (!tv && typeof d.text === 'string') tv = d.text; // fallback if trimming emptied unexpectedly
    setText('text', tv);
  }
  if ('s' in d && 'e' in d && typeof d.s === 'number' && typeof d.e === 'number') {
    setText('window', d.s.toFixed(3) + '–' + d.e.toFixed(3));
  }
  if ('ff_index' in d) setText('ff', String(d.ff_index));
  if ('latency_ms' in d) setText('lat', (d.latency_ms != null ? d.latency_ms : '') + (d.latency_ms != null ? ' ms' : ''));
  if ('rms' in d && typeof d.rms === 'number') setText('rms', d.rms.toFixed(4));
  if ('peak' in d && typeof d.peak === 'number') setText('peak', d.peak.toFixed(4));

  // F0 numbers
  if (typeof d.f0_src_median === 'number') setText('f0src', d.f0_src_median.toFixed(1) + ' Hz');
  if (typeof d.f0_mic_median === 'number') setText('f0mic', d.f0_mic_median.toFixed(1) + ' Hz');
  if (typeof d.voiced_src === 'number') setText('voiced', Math.round(d.voiced_src * 100) + '%');

  // Pitch graph from source series
  if (Array.isArray(d.f0_src_series)) {
    drawPitch(d.f0_src_series, d.f0_src_median);
  }

  // Reset buttons to Play when new analysis/mic update arrives
  var btn = document.getElementById('play-button');
  if (btn) btn.textContent = 'Play';
  var btnMic = document.getElementById('play-mic-button');
  if (btnMic) btnMic.textContent = 'Play';
  var btnBoth = document.getElementById('play-both-button');
  if (btnBoth) btnBoth.textContent = 'Play';

  // Source player: prefer latest_path if present
  var player = document.getElementById('player');
  if (player && (d.latest_path || d.out_path)) {
    var chosen = d.latest_path || d.out_path;
    var url = 'file:///' + String(chosen).replace(/\\/g, '/');
    var ts = Date.now();
    player.src = encodeURI(url + '?t=' + ts);
    try { player.pause(); player.currentTime = 0; } catch (_) {}
    player.load();
  }

  // Mic player: update when latest_mic_path present
  var playerMic = document.getElementById('player-mic');
  if (playerMic && d.latest_mic_path) {
    var urlm = 'file:///' + String(d.latest_mic_path).replace(/\\/g, '/');
    var ts2 = Date.now();
    playerMic.src = encodeURI(urlm + '?t=' + ts2);
    try { playerMic.pause(); playerMic.currentTime = 0; } catch (_) {}
    playerMic.load();
  }
});

function togglePlayGroup(players, buttons) {
  if (!buttons || !buttons.length) return;
  var isPlay = buttons[0] && buttons[0].textContent === 'Play';
  if (isPlay) {
    players.forEach(function(p){ try { if (p) p.currentTime = 0; } catch(_){} });
    players.forEach(function(p){
      if (p && typeof p.play === 'function') {
        var pr = p.play();
        if (pr && typeof pr.then === 'function') pr.catch(function(){});
      }
    });
    buttons.forEach(function(b){ if (b) b.textContent = 'Pause'; });
  } else {
    players.forEach(function(p){ if (p && typeof p.pause === 'function') p.pause(); });
    buttons.forEach(function(b){ if (b) b.textContent = 'Play'; });
  }
}

function togglePlay() {
  togglePlayGroup(
    [document.getElementById('player')],
    [document.getElementById('play-button'), document.getElementById('play-both-button')]
  );
}

// When playback naturally reaches the end, revert UI state to Play
(function attachEndedHandler() {
  var player = document.getElementById('player');
  if (!player) return;
  player.addEventListener('ended', function () {
    var button = document.getElementById('play-button');
    if (button) button.textContent = 'Play';
    var buttonMic = document.getElementById('play-mic-button');
    if (buttonMic) buttonMic.textContent = 'Play';
    var buttonBoth = document.getElementById('play-both-button');
    if (buttonBoth) buttonBoth.textContent = 'Play';
  });
})();

function togglePlayMic() {
  togglePlayGroup(
    [document.getElementById('player-mic')],
    [document.getElementById('play-mic-button'), document.getElementById('play-both-button')]
  );
}

(function attachEndedHandlerMic() {
  var player = document.getElementById('player-mic');
  if (!player) return;
  player.addEventListener('ended', function () {
    var button = document.getElementById('play-mic-button');
    if (button) button.textContent = 'Play';
    var buttonSrc = document.getElementById('play-button');
    if (buttonSrc) buttonSrc.textContent = 'Play';
    var buttonBoth = document.getElementById('play-both-button');
    if (buttonBoth) buttonBoth.textContent = 'Play';
  });
})();

(function setupMicSelector() {
  var sel = document.getElementById('mic-selector');
  if (!sel) return;
  var saved = localStorage.getItem('mic_device') || 'default';
  sel.value = saved;
  sel.addEventListener('change', function () {
    localStorage.setItem('mic_device', sel.value);
    try {
      if (window.ipc && typeof window.ipc.postMessage === 'function') {
        window.ipc.postMessage(JSON.stringify({ type: 'mic_device', value: sel.value }));
      }
    } catch (_) {}
  });
})();

window.addEventListener('devices', function (e) {
  var sel = document.getElementById('mic-selector');
  if (!sel) return;
  var list = (e.detail && e.detail.micDevices) || [];
  var current = localStorage.getItem('mic_device') || 'default';
  var opts = [{ id: 'default', name: 'Default' }].concat(list.map(function (it) {
    if (typeof it === 'string') return { id: it, name: it.replace(/^audio=*/, '') };
    return { id: it.id, name: it.name || it.id };
  }));
  sel.innerHTML = opts.map(function (o) {
    return '<option value="' + o.id + '">' + o.name + '</option>';
  }).join('');
  sel.value = current;
  if (sel.value !== current) {
    sel.value = 'default';
    localStorage.setItem('mic_device', 'default');
  }
});

window.addEventListener('play-both', function (e) {
  togglePlayBoth();
});

function togglePlayBoth() {
  togglePlayGroup(
    [document.getElementById('player'), document.getElementById('player-mic')],
    [
      document.getElementById('play-both-button'),
      document.getElementById('play-button'),
      document.getElementById('play-mic-button')
    ]
  );
}


window.addEventListener('analysis', e => {
  const d = e.detail||{};
  console.log('analysis:',
    'seriesLen=', Array.isArray(d.f0_src_series) ? d.f0_src_series.length : null,
    'median=', d.f0_src_median,
    'first10=', (d.f0_src_series||[]).slice(0,10)
  );
});