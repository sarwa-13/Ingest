/* ══════════════════════════════════════════════════
   INGEST – Renderer
   Vanilla JS, no framework. Wired to window.ingest IPC bridge.
══════════════════════════════════════════════════ */

/* ── Screen management ───────────────────────────── */
function showScreen(id) {
  document.querySelectorAll('.screen').forEach(s => s.classList.add('hidden'));
  const el = document.getElementById(id);
  if (el) el.classList.remove('hidden');
}

window.ingest.onReady(({ screen }) => {
  if (screen === 'main') {
    showScreen('screen-main');
    initMain();
  } else {
    showScreen('screen-setup');
    initSetup();
  }
});

/* ══════════════════════════════════════════════════
   SETUP SCREEN
══════════════════════════════════════════════════ */
function initSetup() {
  const btn     = document.getElementById('setup-start-btn');
  const wrap    = document.getElementById('setup-progress-wrap');
  const label   = document.getElementById('setup-label');
  const barFill = document.getElementById('setup-bar');
  const pct     = document.getElementById('setup-pct');

  window.ingest.onSetupProgress(({ label: l, pct: p }) => {
    wrap.classList.remove('hidden');
    label.textContent    = l;
    barFill.style.width  = p + '%';
    pct.textContent      = p + '%';
  });

  btn.addEventListener('click', async () => {
    btn.disabled       = true;
    btn.textContent    = 'Setting up…';
    wrap.classList.remove('hidden');
    label.textContent  = 'Connecting to GitHub…';

    const result = await window.ingest.startSetup();

    if (result.success) {
      label.textContent = 'Done! Launching…';
      setTimeout(() => {
        showScreen('screen-main');
        initMain();
      }, 700);
    } else {
      label.textContent        = 'Error: ' + result.error;
      barFill.style.background = '#c46969';
      btn.disabled             = false;
      btn.textContent          = 'Try Again';
    }
  });
}

/* ══════════════════════════════════════════════════
   MAIN APP
══════════════════════════════════════════════════ */

/* ── URL validators ──────────────────────────────── */
const URL_PATTERNS = {
  youtube:   /youtu\.?be/,
  instagram: /instagram\.com/,
};

function looksValid(url, platform) {
  const t = url.trim().toLowerCase();
  if (!/^https?:\/\//.test(t)) return false;
  return URL_PATTERNS[platform]?.test(t) ?? false;
}

/* ── Quality → yt-dlp format string ─────────────── */
const QUALITY_MAP = {
  '2160p':    'bestvideo[height<=2160]+bestaudio/best',
  '1440p':    'bestvideo[height<=1440]+bestaudio/best',
  '1080p':    'bestvideo[height<=1080]+bestaudio/best',
  '720p':     'bestvideo[height<=720]+bestaudio/best',
  '480p':     'bestvideo[height<=480]+bestaudio/best',
  'Original': 'bestvideo+bestaudio/best',
};

// Instagram serves content as a single combined mp4 stream (not separate DASH).
// Using best[ext=mp4] grabs the combined stream directly — no merge step needed.
// This avoids both the "no video track" bug and the orphaned .m4a file bug.
const IG_QUALITY_MAP = {
  '2160p':    'best[height<=2160][ext=mp4]/best[height<=2160]/best',
  '1440p':    'best[height<=1440][ext=mp4]/best[height<=1440]/best',
  '1080p':    'best[height<=1080][ext=mp4]/best[height<=1080]/best',
  '720p':     'best[height<=720][ext=mp4]/best[height<=720]/best',
  '480p':     'best[height<=480][ext=mp4]/best[height<=480]/best',
  'Original': 'best[ext=mp4]/best',
};

/* ── Quality auto-detection helpers ─────────────── */
function heightToQuality(height, platform) {
  if (platform === 'instagram') return height > 1080 ? 'Original' : '1080p';
  if (height >= 2160) return '2160p';
  if (height >= 1440) return '1440p';
  if (height >= 1080) return '1080p';
  if (height >= 720)  return '720p';
  return '480p';
}

function abrToBitrate(abr) {
  if (abr >= 300) return '320 kbps';
  if (abr >= 230) return '256 kbps';
  if (abr >= 160) return '192 kbps';
  return '128 kbps';
}

function setSelectDropdownValue(wrapId, value) {
  const wrap = document.getElementById(wrapId);
  if (!wrap) return;
  const valEl = wrap.querySelector('.sel-btn .sel-val');
  wrap.querySelectorAll('.sel-item').forEach(item => {
    item.classList.toggle('on', item.dataset.value === value);
  });
  const match = wrap.querySelector(`.sel-item[data-value="${value}"]`);
  if (match && valEl) valEl.textContent = value;
}

// Tracks the last URL queued for detection per prefix to discard stale results
const _detectingUrl = { yt: null, ig: null };

async function detectAndApplyQuality(url, platform) {
  const prefix = PLATFORMS[platform].prefix;
  _detectingUrl[prefix] = url;

  try {
    const { height, abr } = await window.ingest.detectFormat(url);
    if (_detectingUrl[prefix] !== url) return; // URL changed while we were waiting

    if (height) setSelectDropdownValue(prefix + '-quality-sel', heightToQuality(height, platform));
    if (abr)    setSelectDropdownValue(prefix + '-bitrate-sel', abrToBitrate(abr));
  } catch { /* detection is best-effort */ }
}

/* ── Platform config ─────────────────────────────── */
const PLATFORMS = {
  youtube:   { prefix: 'yt',  name: 'YouTube'   },
  instagram: { prefix: 'ig',  name: 'Instagram' },
};

const PLATFORM_THUMB_COLOR = {
  youtube:   '#1f3826',
  instagram: '#3a1f24',
};

const PLATFORM_ICON_PATH = {
  youtube:   '<rect x="3" y="6" width="18" height="12" rx="3" stroke="currentColor" stroke-width="1.7"/><path d="M11 10v4l3.5-2z" fill="currentColor"/>',
  instagram: '<rect x="4" y="4" width="16" height="16" rx="4.5" stroke="currentColor" stroke-width="1.7"/><circle cx="12" cy="12" r="3.5" stroke="currentColor" stroke-width="1.7"/><circle cx="17" cy="7" r="0.9" fill="currentColor"/>',
};

/* ── State ───────────────────────────────────────── */
const state = {
  tab:       'youtube',
  outputDir: localStorage.getItem('outputDir') || '~/Downloads/Ingest',
  showOutputPanel: false,
  queue: [],
  activeItems: { yt: [], ig: [] },  // queue IDs for the current batch per prefix
  activeIdx:   { yt: 0,  ig: 0  }, // index of item currently being downloaded
  downloading: {
    yt: false,
    ig: false,
  },
};

/* ── Helpers ─────────────────────────────────────── */
function uid() {
  return Math.random().toString(36).slice(2, 9);
}

function shortenPath(p) {
  if (p.length <= 30) return p;
  const parts = p.split('/');
  if (parts.length <= 3) return p;
  return parts[0] + '/…/' + parts.slice(-2).join('/');
}

/* ── Global selects ──────────────────────────────── */
function initSelectDropdown(wrapId) {
  const wrap = document.getElementById(wrapId);
  if (!wrap) return;
  const btn  = wrap.querySelector('.sel-btn');
  const menu = wrap.querySelector('.sel-menu');
  const valEl = btn.querySelector('.sel-val');
  const chevron = btn.querySelector('.chevron');

  function close() {
    menu.classList.add('hidden');
    btn.classList.remove('open');
    if (chevron) chevron.classList.remove('flipped');
  }

  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    const isOpen = !menu.classList.contains('hidden');
    // close all other open dropdowns first
    document.querySelectorAll('.sel-menu:not(.hidden)').forEach(m => {
      m.classList.add('hidden');
      const b = m.closest('.sel-wrap')?.querySelector('.sel-btn');
      if (b) b.classList.remove('open');
      const c = b?.querySelector('.chevron');
      if (c) c.classList.remove('flipped');
    });
    if (isOpen) return;
    menu.classList.remove('hidden');
    btn.classList.add('open');
    if (chevron) chevron.classList.add('flipped');
  });

  menu.querySelectorAll('.sel-item').forEach(item => {
    item.addEventListener('click', (e) => {
      e.stopPropagation();
      const val = item.dataset.value;
      // update label
      if (valEl) valEl.textContent = val;
      // update active state
      menu.querySelectorAll('.sel-item').forEach(i => i.classList.remove('on'));
      item.classList.add('on');
      close();
      // dispatch custom event
      wrap.dispatchEvent(new CustomEvent('change', { detail: { value: val } }));
    });
  });

  document.addEventListener('click', close);

  // expose getValue
  wrap._getValue = () => {
    const on = menu.querySelector('.sel-item.on');
    return on ? on.dataset.value : null;
  };
}

/* ── Segment control ──────────────────────────────── */
function moveSegThumb(seg) {
  const active = seg.querySelector('.seg-btn.on');
  const thumb  = seg.querySelector('.seg-thumb');
  if (!active || !thumb) return;
  thumb.style.left  = active.offsetLeft + 'px';
  thumb.style.width = active.offsetWidth + 'px';
}

function initSegment(segId, onChange) {
  const seg = document.getElementById(segId);
  if (!seg) return;

  const thumb = document.createElement('div');
  thumb.className = 'seg-thumb';
  seg.insertBefore(thumb, seg.firstChild);
  // Position without transition on first paint
  thumb.style.transition = 'none';
  requestAnimationFrame(() => {
    moveSegThumb(seg);
    requestAnimationFrame(() => { thumb.style.transition = ''; });
  });

  seg.querySelectorAll('.seg-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      seg.querySelectorAll('.seg-btn').forEach(b => b.classList.remove('on'));
      btn.classList.add('on');
      moveSegThumb(seg);
      onChange(btn.dataset.value);
    });
  });
  seg._getValue = () => seg.querySelector('.seg-btn.on')?.dataset.value ?? null;
}

/* ── Chip row ─────────────────────────────────────── */
function initChips(rowId) {
  const row = document.getElementById(rowId);
  if (!row) return;
  row.querySelectorAll('.chip').forEach(chip => {
    chip.addEventListener('click', () => {
      row.querySelectorAll('.chip').forEach(c => c.classList.remove('on'));
      chip.classList.add('on');
      row.dispatchEvent(new CustomEvent('change', { detail: { value: chip.dataset.value } }));
    });
  });
  row._getValue = () => row.querySelector('.chip.on')?.dataset.value ?? null;
}

/* ── URL field wiring ─────────────────────────────── */
function initUrlField(platform, inputId, fieldId, statusId, pasteId, clearId, ctaId, ctaLabelId) {
  const input       = document.getElementById(inputId);
  const field       = document.getElementById(fieldId);
  const status      = document.getElementById(statusId);
  const clearBtn    = document.getElementById(clearId);
  const ctaBtn      = document.getElementById(ctaId);
  const addQueueBtn = document.getElementById(PLATFORMS[platform].prefix + '-add-queue');

  function setStatus(type, text) {
    status.innerHTML = '';
    const span = document.createElement('span');
    span.className = 'status-' + type;
    span.textContent = text;
    status.appendChild(span);
  }

  function getValidUrls(val) {
    return val.split('\n').map(u => u.trim()).filter(u => u && looksValid(u, platform));
  }

  let _detectTimer = null;

  function updateField(val) {
    clearBtn.classList.toggle('hidden', !val.trim());
    field.classList.remove('state-ok', 'state-err');
    ctaBtn.disabled = true;

    if (!val.trim()) {
      if (addQueueBtn) addQueueBtn.disabled = true;
      const prefix = PLATFORMS[platform].prefix;
      const queuedCount = state.queue.filter(q => q.prefix === prefix && q.status === 'queued').length;
      ctaBtn.disabled = queuedCount === 0;
      setStatus('hint', queuedCount > 0
        ? `${queuedCount} item${queuedCount > 1 ? 's' : ''} queued — click Download to start`
        : 'Paste URLs to begin — one per line, up to 10');
      return;
    }

    const valid = getValidUrls(val);
    const name  = PLATFORMS[platform]?.name ?? platform;

    if (valid.length === 0) {
      field.classList.add('state-err');
      if (addQueueBtn) addQueueBtn.disabled = true;
      setStatus('err', `No valid ${name} links found`);
    } else if (valid.length > 10) {
      field.classList.add('state-err');
      if (addQueueBtn) addQueueBtn.disabled = true;
      setStatus('err', `Max 10 URLs — remove ${valid.length - 10}`);
    } else {
      field.classList.add('state-ok');
      const s = valid.length === 1 ? '' : 's';
      setStatus('ok', `✓ ${valid.length} URL${s} recognised — ready to download`);
      ctaBtn.disabled = false;
      if (addQueueBtn) addQueueBtn.disabled = false;
      updateCtaLabel(ctaId, ctaLabelId, platform);

      // Auto-detect best quality for a single URL (debounced)
      if (valid.length === 1) {
        clearTimeout(_detectTimer);
        _detectTimer = setTimeout(() => detectAndApplyQuality(valid[0], platform), 400);
      }
    }
  }

  input.addEventListener('input', () => updateField(input.value));

  document.getElementById(pasteId).addEventListener('click', async () => {
    try {
      const t = await navigator.clipboard.readText();
      if (!t) return;
      const existing = input.value.trim();
      input.value = existing ? existing + '\n' + t.trim() : t.trim();
      updateField(input.value);
    } catch {
      // clipboard not available
    }
  });

  clearBtn.addEventListener('click', () => {
    input.value = '';
    updateField('');
    input.focus();
  });
}

function updateCtaLabel(ctaId, ctaLabelId, platform) {
  const prefix  = PLATFORMS[platform].prefix;
  const typeEl  = document.getElementById(prefix === 'yt' ? 'yt-type-seg' : prefix === 'ig' ? 'ig-type-seg' : null);
  const type    = typeEl?._getValue?.() ?? 'video';
  const format  = document.getElementById(`${prefix}-format-chips`)?._getValue?.() ?? '';

  let label = 'Download';
  if (type === 'audio') {
    label = 'Download audio · ' + format;
  } else if (type === 'thumbnail') {
    label = 'Download thumbnail · ' + (format || 'JPG');
  } else if (type === 'image') {
    label = 'Download image';
  } else {
    const quality = document.getElementById(`${prefix}-quality-sel`)?._getValue?.() ?? '';
    label = `Download · ${quality} ${format}`.trim();
  }

  document.getElementById(ctaLabelId).textContent = label;
}

// Keep the Download button label in sync with every option change (type seg,
// format chip, quality/bitrate dropdown). Uses delegation so chips rebuilt by
// switchType are automatically covered.
function wireOptionUpdates(platform, ctaId, ctaLabelId) {
  const panel = document.getElementById('pv-' + platform);
  if (!panel) return;
  const refresh = () => updateCtaLabel(ctaId, ctaLabelId, platform);
  panel.addEventListener('click', (e) => {
    if (e.target.closest('.chip, .seg-btn')) refresh();
  });
  // Dropdowns dispatch a custom 'change' on .sel-wrap; it doesn't bubble, so wire each.
  panel.querySelectorAll('.sel-wrap').forEach(w => w.addEventListener('change', refresh));
  refresh();
}

/* ── YouTube type toggle ─────────────────────────── */
function initYtTypeSwitch() {
  const qualityRow  = document.getElementById('yt-quality-row');
  const bitrateRow  = document.getElementById('yt-bitrate-row');
  const formatChips = document.getElementById('yt-format-chips');

  const VIDEO_FORMATS = ['MP4', 'WEBM', 'MOV'];
  const AUDIO_FORMATS = ['MP3', 'M4A', 'WAV', 'OPUS'];
  const THUMB_FORMATS = ['JPG', 'PNG', 'WEBP'];

  function switchType(type) {
    qualityRow.classList.add('hidden');
    bitrateRow.classList.add('hidden');

    if (type === 'audio') {
      bitrateRow.classList.remove('hidden');
      formatChips.innerHTML = AUDIO_FORMATS.map((f, i) =>
        `<button class="chip${i === 0 ? ' on' : ''}" data-value="${f}">${f}</button>`
      ).join('');
    } else if (type === 'thumbnail') {
      formatChips.innerHTML = THUMB_FORMATS.map((f, i) =>
        `<button class="chip${i === 0 ? ' on' : ''}" data-value="${f}">${f}</button>`
      ).join('');
    } else {
      qualityRow.classList.remove('hidden');
      formatChips.innerHTML = VIDEO_FORMATS.map((f, i) =>
        `<button class="chip${i === 0 ? ' on' : ''}" data-value="${f}">${f}</button>`
      ).join('');
    }
    initChips('yt-format-chips');
  }

  initSegment('yt-type-seg', switchType);
}

/* ── Instagram type toggle ───────────────────────── */
function initIgTypeSwitch() {
  const qualityRow  = document.getElementById('ig-quality-row');
  const bitrateRow  = document.getElementById('ig-bitrate-row');
  const formatRow   = document.getElementById('ig-format-row');
  const formatChips = document.getElementById('ig-format-chips');

  const VIDEO_FORMATS = ['MP4', 'MOV'];
  const AUDIO_FORMATS = ['MP3', 'M4A', 'WAV'];
  const THUMB_FORMATS = ['JPG', 'PNG', 'WEBP'];

  function switchType(type) {
    qualityRow.classList.add('hidden');
    bitrateRow.classList.add('hidden');
    formatRow.classList.remove('hidden');

    if (type === 'audio') {
      bitrateRow.classList.remove('hidden');
      formatChips.innerHTML = AUDIO_FORMATS.map((f, i) =>
        `<button class="chip${i === 0 ? ' on' : ''}" data-value="${f}">${f}</button>`
      ).join('');
      initChips('ig-format-chips');
    } else if (type === 'thumbnail') {
      formatChips.innerHTML = THUMB_FORMATS.map((f, i) =>
        `<button class="chip${i === 0 ? ' on' : ''}" data-value="${f}">${f}</button>`
      ).join('');
      initChips('ig-format-chips');
    } else {
      qualityRow.classList.remove('hidden');
      formatChips.innerHTML = VIDEO_FORMATS.map((f, i) =>
        `<button class="chip${i === 0 ? ' on' : ''}" data-value="${f}">${f}</button>`
      ).join('');
      initChips('ig-format-chips');
    }
  }

  initSegment('ig-type-seg', switchType);
}

/* ══════════════════════════════════════════════════
   QUEUE
══════════════════════════════════════════════════ */
function buildMeta(opts) {
  if (opts.type === 'thumbnail') return `Thumbnail · ${(opts.format ?? 'jpg').toUpperCase()}`;
  if (opts.type === 'audio')     return `Audio · ${(opts.format ?? '').toUpperCase()}`;
  return (opts.format ?? 'mp4').toUpperCase();
}

function addQueueItem(platform, url, meta, initialStatus = 'queued', opts = null) {
  const id = uid();
  const prefix = PLATFORMS[platform].prefix;

  state.queue.unshift({
    id,
    platform,
    prefix,
    url,
    title: url,
    meta,
    opts,       // stored per-item so each download uses its own settings
    progress: 0,
    status: initialStatus,
  });

  return id;
}

function updateQueueItem(id, updates) {
  const idx = state.queue.findIndex(q => q.id === id);
  if (idx !== -1) {
    Object.assign(state.queue[idx], updates);
    renderQueueItem(state.queue[idx]);
    renderQueueHead();
  }
}

function renderQueue() {
  const empty  = document.getElementById('queue-empty');
  const panel  = document.getElementById('queue-panel');
  const list   = document.getElementById('q-list');

  if (state.queue.length === 0) {
    empty.classList.remove('hidden');
    panel.classList.add('hidden');
    return;
  }

  empty.classList.add('hidden');
  panel.classList.remove('hidden');
  list.innerHTML = '';
  state.queue.forEach(item => {
    list.appendChild(buildQueueRow(item));
  });

  renderQueueHead();
}

function renderQueueHead() {
  const active = state.queue.filter(q => q.status === 'active').length;
  const done   = state.queue.filter(q => q.status === 'done').length;
  const countEl = document.getElementById('q-count');
  const clearBtn = document.getElementById('q-clear-done');

  const parts = [];
  if (active > 0) parts.push(`<span class="q-dot-active"></span>${active} active`);
  if (done   > 0) parts.push(`${done} done`);
  countEl.innerHTML = parts.join(' <span style="opacity:.4">·</span> ');

  if (done > 0) clearBtn.classList.remove('hidden');
  else          clearBtn.classList.add('hidden');
}

function renderQueueItem(item) {
  const existing = document.getElementById('qr-' + item.id);
  if (!existing) return renderQueue(); // full re-render if not found
  const newRow = buildQueueRow(item);
  existing.replaceWith(newRow);
  renderQueueHead();
}

function buildQueueRow(item) {
  const div = document.createElement('div');
  div.id        = 'qr-' + item.id;
  div.className = 'qr ' + item.status + ' platform-' + (item.platform || 'unknown');

  const statusText = item.status === 'done'   ? 'Saved'
                   : item.status === 'error'  ? 'Failed'
                   : item.status === 'queued' ? 'Queued'
                   : item.progress + '%';

  const thumbColor = PLATFORM_THUMB_COLOR[item.platform] ?? '#1a2a1f';
  const iconPath   = PLATFORM_ICON_PATH[item.platform] ?? '';
  const metaStr = item.meta ? item.meta : item.platform;

  div.innerHTML = `
    <div class="qr-thumb" style="background:${thumbColor}">
      <svg style="position:absolute;inset:0;width:100%;height:100%;padding:5px;color:rgba(255,255,255,0.45)" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">${iconPath}</svg>
    </div>
    <div class="qr-main">
      <div class="qr-top">
        <span class="qr-title">${escHtml(truncate(item.title, 52))}</span>
        <span class="qr-status">${escHtml(statusText)}</span>
      </div>
      <div class="qr-bar"><div class="qr-fill" style="width:${item.progress}%"></div></div>
      <div class="qr-meta">
        <span class="qr-platform">${escHtml(item.platform)}</span>
        <span class="qr-sep">·</span>
        <span>${escHtml(metaStr)}</span>
      </div>
    </div>
    <div class="qr-actions">
      <button class="qr-btn qr-remove" title="Remove">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
      </button>
    </div>`;

  div.querySelector('.qr-remove').addEventListener('click', () => {
    state.queue = state.queue.filter(q => q.id !== item.id);
    renderQueue();
    // Re-evaluate the Download button for this item's platform
    document.getElementById(item.prefix + '-url')?.dispatchEvent(new Event('input'));
  });

  return div;
}

function truncate(s, n) {
  return s.length > n ? s.slice(0, n - 1) + '…' : s;
}

function escHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/* ══════════════════════════════════════════════════
   OUTPUT PANEL
══════════════════════════════════════════════════ */
function initOutputPanel() {
  const pill     = document.getElementById('output-pill');
  const panel    = document.getElementById('output-panel');
  const closeBtn = document.getElementById('op-close');
  const pathDisplay = document.getElementById('op-dir-display');
  const pillPath = document.getElementById('output-pill-path');

  // restore saved dir
  pathDisplay.textContent = state.outputDir;
  pillPath.textContent    = shortenPath(state.outputDir);
  document.getElementById('op-dir-display').textContent = state.outputDir;

  function setDir(dir) {
    state.outputDir = dir;
    localStorage.setItem('outputDir', dir);
    pathDisplay.textContent = dir;
    pillPath.textContent    = shortenPath(dir);
    // update preset highlights
    document.querySelectorAll('.preset').forEach(p => {
      p.classList.toggle('on', p.dataset.path === dir);
    });
  }

  pill.addEventListener('click', () => {
    state.showOutputPanel = !state.showOutputPanel;
    panel.classList.toggle('hidden', !state.showOutputPanel);
    pill.classList.toggle('on', state.showOutputPanel);
  });

  closeBtn.addEventListener('click', () => {
    state.showOutputPanel = false;
    panel.classList.add('hidden');
    pill.classList.remove('on');
  });

  document.getElementById('op-browse').addEventListener('click', async () => {
    const folder = await window.ingest.selectFolder();
    if (folder) setDir(folder);
  });

  document.getElementById('op-open-folder').addEventListener('click', () => {
    window.ingest.openFolder(state.outputDir);
  });

  document.querySelectorAll('.preset').forEach(btn => {
    btn.addEventListener('click', () => setDir(btn.dataset.path));
  });

  // highlight correct preset on load
  document.querySelectorAll('.preset').forEach(p => {
    p.classList.toggle('on', p.dataset.path === state.outputDir);
  });
}

/* ══════════════════════════════════════════════════
   DOWNLOAD
══════════════════════════════════════════════════ */
function buildOpts(platform) {
  if (platform === 'youtube') {
    const type    = document.getElementById('yt-type-seg')?._getValue?.() ?? 'video';
    const format  = document.getElementById('yt-format-chips')?._getValue?.() ?? 'MP4';
    const quality = document.getElementById('yt-quality-sel')?._getValue?.() ?? '1080p';
    const bitrate = document.getElementById('yt-bitrate-sel')?._getValue?.() ?? '256 kbps';

    if (type === 'thumbnail') {
      return { type: 'thumbnail', format: format.toLowerCase(), template: '%(title)s.%(ext)s' };
    }
    return {
      type,
      quality:  type === 'audio' ? 'bestaudio/best' : (QUALITY_MAP[quality] ?? QUALITY_MAP['1080p']),
      format:   format.toLowerCase(),
      bitrate,
      template: '%(title)s.%(ext)s',
    };
  }

  if (platform === 'instagram') {
    const type    = document.getElementById('ig-type-seg')?._getValue?.() ?? 'video';
    const format  = document.getElementById('ig-format-chips')?._getValue?.() ?? 'MP4';
    const quality = document.getElementById('ig-quality-sel')?._getValue?.() ?? 'Original';
    const bitrate = document.getElementById('ig-bitrate-sel')?._getValue?.() ?? '192 kbps';

    if (type === 'thumbnail') {
      return { type: 'thumbnail', format: format.toLowerCase(), template: '%(uploader)s_%(title)s.%(ext)s' };
    }
    return {
      type,
      quality:  type === 'audio' ? 'bestaudio/best' : (IG_QUALITY_MAP[quality] ?? IG_QUALITY_MAP['Original']),
      format:   format.toLowerCase(),
      bitrate,
      template: '%(uploader)s_%(title)s.%(ext)s',
    };
  }

  return {};
}

function setDownloading(prefix, active) {
  state.downloading[prefix] = active;

  const dlBtn     = document.getElementById(prefix + '-dl');
  const cancelBtn = document.getElementById(prefix + '-cancel');

  if (dlBtn)     dlBtn.disabled     = active;
  if (dlBtn)     dlBtn.classList.toggle('downloading', active);
  if (cancelBtn) cancelBtn.classList.toggle('hidden', !active);
}

// Stores URLs+settings into the queue without starting a download.
function addToQueue(platform) {
  const prefix = PLATFORMS[platform].prefix;
  const input  = document.getElementById(prefix + '-url');
  const raw    = input?.value ?? '';

  const urls = raw.split('\n')
    .map(u => u.trim())
    .filter(u => u && looksValid(u, platform))
    .slice(0, 10);

  if (!urls.length) return;

  const opts = buildOpts(platform);
  const meta = buildMeta(opts);
  const ids  = urls.map(url => addQueueItem(platform, url, meta, 'queued', opts));
  renderQueue();

  // If a download is already running for this platform, hot-enqueue into the
  // active job so items are picked up automatically without pressing Download.
  input.value = '';
  input.dispatchEvent(new Event('input'));

  if (state.downloading[prefix]) {
    // Hot-enqueue into the running job — it picks items up automatically.
    state.activeItems[prefix] = [...(state.activeItems[prefix] ?? []), ...ids];
    window.ingest.enqueueDownload({ prefix, items: urls.map(url => ({ url, opts })) });
  } else {
    // Auto-start this platform's download immediately.
    startDownload(platform);
  }
}

// Downloads all queued items for this platform.
// If the URL input has content, those are added first with the current settings.
function startDownload(platform) {
  const prefix = PLATFORMS[platform].prefix;

  // If a job is already running for this platform, route through addToQueue so the
  // new URLs hot-enqueue into the live job instead of cancelling and restarting it.
  if (state.downloading[prefix]) {
    addToQueue(platform);
    return;
  }

  const input  = document.getElementById(prefix + '-url');
  const raw    = input?.value ?? '';

  // Flush any URLs still in the input into the queue
  const urls = raw.split('\n')
    .map(u => u.trim())
    .filter(u => u && looksValid(u, platform))
    .slice(0, 10);

  if (urls.length) {
    const opts = buildOpts(platform);
    const meta = buildMeta(opts);
    urls.forEach(url => addQueueItem(platform, url, meta, 'queued', opts));
    input.value = '';
    input.dispatchEvent(new Event('input'));
  }

  // Collect all queued items oldest-first (queue.unshift means index 0 is newest)
  const queued = state.queue
    .filter(q => q.prefix === prefix && q.status === 'queued')
    .reverse();

  if (!queued.length) return;

  const dir  = state.outputDir;
  const ids  = queued.map(q => q.id);
  state.activeItems[prefix] = ids;
  state.activeIdx[prefix]   = 0;
  renderQueue();

  setDownloading(prefix, true);

  const items = queued.map(q => ({ url: q.url, opts: q.opts }));
  window.ingest.startDownload({ prefix, items, dir });

  // Also kick off any other platforms that have queued items but aren't running yet.
  for (const [p, cfg] of Object.entries(PLATFORMS)) {
    if (p === platform) continue;
    const op = cfg.prefix;
    if (state.downloading[op]) continue;
    const otherQueued = state.queue.filter(q => q.prefix === op && q.status === 'queued');
    if (otherQueued.length) startDownload(p);
  }
}

/* ══════════════════════════════════════════════════
   IPC LISTENERS (global, after main init)
══════════════════════════════════════════════════ */
function initIpcListeners() {

  window.ingest.onLog(({ prefix, text }) => {
    const ids = state.activeItems[prefix];
    const id  = ids?.[state.activeIdx[prefix] ?? 0];
    if (!id) return;

    const destMatch = text.match(/\[download\]\s+Destination:\s+(.+)/);
    if (destMatch) {
      const filename = destMatch[1].split('/').pop().replace(/\.[^.]+$/, '');
      if (filename) updateQueueItem(id, { title: filename });
    }

    const titleMatch = text.match(/\[info\]\s+[^:]+:\s+(.+)/);
    if (titleMatch && titleMatch[1].length < 120) {
      updateQueueItem(id, { title: titleMatch[1] });
    }

    const foundMatch = text.match(/♪ Found: (.+)/);
    if (foundMatch) {
      updateQueueItem(id, { title: foundMatch[1] });
    }
  });

  window.ingest.onProgress(({ prefix, pct }) => {
    const ids = state.activeItems[prefix];
    if (!ids || !ids.length) return;
    const idx = state.activeIdx[prefix] ?? 0;
    const id  = ids[idx];
    if (!id) return;
    // overall = (idx + itemPct/100) / n * 100  →  itemPct = (pct/100 * n - idx) * 100
    const itemPct = Math.round(((pct / 100) * ids.length - idx) * 100);
    updateQueueItem(id, { progress: Math.max(0, Math.min(99, itemPct)) });
  });

  window.ingest.onItemStatus(({ prefix, index, status }) => {
    const ids = state.activeItems[prefix];
    if (!ids || index >= ids.length) return;
    const id = ids[index];
    if (status === 'active') state.activeIdx[prefix] = index;
    const statusMap = { active: 'active', done: 'done', error: 'error' };
    updateQueueItem(id, { status: statusMap[status] ?? 'active' });
  });

  window.ingest.onComplete(({ prefix, success }) => {
    const ids = state.activeItems[prefix] ?? [];
    ids.forEach(id => {
      const item = state.queue.find(q => q.id === id);
      if (item && item.status === 'active') {
        updateQueueItem(id, { status: success ? 'done' : 'error', progress: success ? 100 : 0 });
      }
    });
    state.activeItems[prefix] = [];
    state.activeIdx[prefix]   = 0;
    setDownloading(prefix, false);
    // Re-evaluate Download button — enables it if more queued items remain
    document.getElementById(prefix + '-url')?.dispatchEvent(new Event('input'));
  });
}

/* ══════════════════════════════════════════════════
   INIT MAIN
══════════════════════════════════════════════════ */
function initMain() {

  // ── yt-dlp version badge ─────────────────────
  const badge = document.getElementById('ver-badge');
  window.ingest.checkYtDlp().then(({ found, version }) => {
    if (found) {
      badge.textContent = 'yt-dlp ' + version;
      badge.classList.add('ok');
    } else {
      badge.textContent = 'yt-dlp missing';
      badge.classList.add('err');
    }
  });

  // ── yt-dlp update toast ──────────────────────
  window.ingest.onYtDlpUpdateAvailable(({ currentVersion, latestVersion, downloadUrl }) => {
    const toast = document.getElementById('toast-ytdlp');
    document.getElementById('toast-ytdlp-versions').textContent =
      `${currentVersion} → ${latestVersion}`;
    toast.classList.remove('hidden');

    const updateBtn = document.getElementById('toast-ytdlp-update');
    updateBtn.addEventListener('click', async () => {
      updateBtn.textContent = 'Updating…';
      updateBtn.disabled    = true;

      window.ingest.onYtDlpUpdateProgress(({ pct }) => {
        updateBtn.textContent = pct + '%';
      });

      const result = await window.ingest.doYtDlpUpdate({ downloadUrl });
      if (result.success) {
        toast.classList.add('hidden');
        badge.textContent  = 'yt-dlp ' + latestVersion;
        badge.style.color  = 'var(--moss-bright)';
        setTimeout(() => { badge.style.color = ''; }, 2000);
      } else {
        updateBtn.textContent = 'Failed';
      }
    }, { once: true });
  });

  document.getElementById('toast-ytdlp-later').addEventListener('click', () => {
    document.getElementById('toast-ytdlp').classList.add('hidden');
  });

  // ── App update toast ─────────────────────────
  let _appUpdateUrl = '';
  window.ingest.onAppUpdate(info => {
    const toast = document.getElementById('toast-app');
    document.getElementById('toast-app-ver').textContent = 'v' + info.latestVersion + ' available';
    _appUpdateUrl = info.downloadUrl || '';
    toast.classList.remove('hidden');
  });

  document.getElementById('toast-app-download').addEventListener('click', async () => {
    if (_appUpdateUrl) await window.ingest.downloadAppUpdate(_appUpdateUrl);
  });

  document.getElementById('toast-app-later').addEventListener('click', () => {
    document.getElementById('toast-app').classList.add('hidden');
  });

  // ── Tab slider ───────────────────────────────
  const tabsNav   = document.getElementById('tabs-nav');
  const tabSlider = document.createElement('div');
  tabSlider.className = 'tab-slider';
  tabsNav.appendChild(tabSlider);

  function moveTabSlider(activeBtn) {
    tabSlider.style.left  = activeBtn.offsetLeft + 'px';
    tabSlider.style.width = activeBtn.offsetWidth + 'px';
  }

  // Position without transition on first paint
  tabSlider.style.transition = 'none';
  requestAnimationFrame(() => {
    const initial = tabsNav.querySelector('.tab.on');
    if (initial) moveTabSlider(initial);
    requestAnimationFrame(() => { tabSlider.style.transition = ''; });
  });

  // ── Tab switching ────────────────────────────
  document.querySelectorAll('.tab').forEach(btn => {
    btn.addEventListener('click', () => {
      const tab = btn.dataset.tab;
      state.tab = tab;
      document.querySelectorAll('.tab').forEach(t => t.classList.remove('on'));
      btn.classList.add('on');
      moveTabSlider(btn);
      document.querySelectorAll('.pv').forEach(p => p.classList.add('hidden'));
      document.getElementById('pv-' + tab)?.classList.remove('hidden');
      // Re-position seg thumbs now that the panel is visible and has layout
      document.getElementById('pv-' + tab)?.querySelectorAll('.seg').forEach(seg => moveSegThumb(seg));
    });
  });

  // ── Queue clear-done ─────────────────────────
  document.getElementById('q-clear-done').addEventListener('click', () => {
    state.queue = state.queue.filter(q => q.status !== 'done');
    renderQueue();
  });

  // ── Output panel ─────────────────────────────
  initOutputPanel();

  // ── Select dropdowns ─────────────────────────
  ['yt-quality-sel', 'yt-bitrate-sel',
   'ig-quality-sel', 'ig-bitrate-sel'].forEach(id => initSelectDropdown(id));

  // ── YouTube ──────────────────────────────────
  initYtTypeSwitch();
  initChips('yt-format-chips');

  initUrlField('youtube', 'yt-url', 'yt-url-field', 'yt-url-status',
               'yt-paste', 'yt-url-clear', 'yt-dl', 'yt-cta-label');
  wireOptionUpdates('youtube', 'yt-dl', 'yt-cta-label');

  document.getElementById('yt-dl').addEventListener('click', () => startDownload('youtube'));
  document.getElementById('yt-add-queue').addEventListener('click', () => addToQueue('youtube'));
  document.getElementById('yt-cancel').addEventListener('click', async () => {
    await window.ingest.cancelDownload('yt');
    setDownloading('yt', false);
    state.activeItems['yt'].forEach(id => updateQueueItem(id, { status: 'error', title: 'Cancelled' }));
    state.activeItems['yt'] = [];
    state.activeIdx['yt']   = 0;
  });

  // ── Instagram ────────────────────────────────
  initIgTypeSwitch();
  initChips('ig-format-chips');

  initUrlField('instagram', 'ig-url', 'ig-url-field', 'ig-url-status',
               'ig-paste', 'ig-url-clear', 'ig-dl', 'ig-cta-label');
  wireOptionUpdates('instagram', 'ig-dl', 'ig-cta-label');

  document.getElementById('ig-dl').addEventListener('click', () => startDownload('instagram'));
  document.getElementById('ig-add-queue').addEventListener('click', () => addToQueue('instagram'));
  document.getElementById('ig-cancel').addEventListener('click', async () => {
    await window.ingest.cancelDownload('ig');
    setDownloading('ig', false);
    state.activeItems['ig'].forEach(id => updateQueueItem(id, { status: 'error', title: 'Cancelled' }));
    state.activeItems['ig'] = [];
    state.activeIdx['ig']   = 0;
  });

  // ── IPC listeners ────────────────────────────
  initIpcListeners();
}
