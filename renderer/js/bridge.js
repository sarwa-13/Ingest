(function () {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  window.ingest = {
    onReady: function (cb) { return invoke('get_initial_state').then(cb); },
    startSetup: function () { return invoke('setup_start'); },
    onSetupProgress: function (cb) { return listen('setup:progress', function (e) { cb(e.payload); }); },
    checkYtDlp: function () { return invoke('ytdlp_check'); },
    doYtDlpUpdate: function (d) { return invoke('ytdlp_do_update', { downloadUrl: d.downloadUrl }); },
    onYtDlpUpdateAvailable: function (cb) { return listen('ytdlp:update-available', function (e) { cb(e.payload); }); },
    onYtDlpUpdateProgress: function (cb) { return listen('ytdlp:update-progress', function (e) { cb(e.payload); }); },
    startDownload: function (p) { return invoke('download_start', { payload: p }); },
    cancelDownload: function () { return invoke('download_cancel'); },
    enqueueDownload: function (p) { return invoke('download_enqueue', { prefix: p.prefix, items: p.items }); },
    onLog: function (cb) { return listen('download:log', function (e) { cb(e.payload); }); },
    onProgress: function (cb) { return listen('download:progress', function (e) { cb(e.payload); }); },
    onItemStatus: function (cb) { return listen('download:item-status', function (e) { cb(e.payload); }); },
    onComplete: function (cb) { return listen('download:complete', function (e) { cb(e.payload); }); },
    selectFolder: function () { return invoke('dialog_folder'); },
    openFolder: function (p) { return invoke('shell_open_folder', { path: p }); },
    detectFormat: function (url) { return invoke('format_detect', { url: url }); },
    onAppUpdate: function (cb) { return listen('app:update-available', function (e) { cb(e.payload); }); },
    onAppDownloaded: function (cb) { return listen('app:update-downloaded', function (e) { cb(e.payload); }); },
    checkAppUpdate: function () { return invoke('app_check_update'); },
    downloadAppUpdate: function (url) { return invoke('app_download_update', { url: url }); },
    installAppUpdate: function () { return invoke('app_install_update'); },
  };
})();
