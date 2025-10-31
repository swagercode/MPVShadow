function setText(id, value) {
  var el = document.getElementById(id);
  if (el) el.textContent = value ?? '';
}

window.addEventListener('analysis', function (e) {
  var d = e.detail || {};
  setText('text', d.text);
  if (typeof d.s === 'number' && typeof d.e === 'number') {
    setText('window', d.s.toFixed(3) + '–' + d.e.toFixed(3));
  } else {
    setText('window', '');
  }
  setText('ff', String(d.ff_index));
  setText('lat', (d.latency_ms != null ? d.latency_ms : '') + (d.latency_ms != null ? ' ms' : ''));
  if (typeof d.rms === 'number') setText('rms', d.rms.toFixed(4));
  if (typeof d.peak === 'number') setText('peak', d.peak.toFixed(4));
  setText('out', d.out_path);
  // Reset button to Play whenever a new analysis arrives
  var btn = document.getElementById('play-button');
  if (btn) btn.textContent = 'Play';

  // Point the audio element at the new WAV (file:/// path)
  var player = document.getElementById('player');
  if (player && d.out_path) {
    var url = 'file:///' + String(d.out_path).replace(/\\/g, '/');
    player.src = encodeURI(url);
    try { player.pause(); player.currentTime = 0; } catch (_) {}
    player.load();
  }
});

function togglePlay() {
  var button = document.getElementById('play-button');
  var player = document.getElementById('player');
  if (!button || !player) return;
  if (button.textContent === 'Play') {
    try { player.currentTime = 0; } catch (_) {}
    var playPromise = player.play();
    if (playPromise && typeof playPromise.then === 'function') {
      playPromise.catch(function(){});
    }
    button.textContent = 'Pause';
  } else {
    player.pause();
    button.textContent = 'Play';
  }
}

// When playback naturally reaches the end, revert UI state to Play
(function attachEndedHandler() {
  var player = document.getElementById('player');
  if (!player) return;
  player.addEventListener('ended', function () {
    var button = document.getElementById('play-button');
    if (button) button.textContent = 'Play';
  });
})();
