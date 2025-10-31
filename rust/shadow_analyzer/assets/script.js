function setText(id, value) {
  var el = document.getElementById(id);
  if (el) el.textContent = value ?? '';
}

window.addEventListener('analysis', function (e) {
  var d = e.detail || {};
  if ('text' in d) setText('text', d.text);
  if ('s' in d && 'e' in d && typeof d.s === 'number' && typeof d.e === 'number') {
    setText('window', d.s.toFixed(3) + '–' + d.e.toFixed(3));
  }
  if ('ff_index' in d) setText('ff', String(d.ff_index));
  if ('latency_ms' in d) setText('lat', (d.latency_ms != null ? d.latency_ms : '') + (d.latency_ms != null ? ' ms' : ''));
  if ('rms' in d && typeof d.rms === 'number') setText('rms', d.rms.toFixed(4));
  if ('peak' in d && typeof d.peak === 'number') setText('peak', d.peak.toFixed(4));
  if ('out_path' in d) setText('out', d.out_path);

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
