// The tab screen-share stream behind the Robot's `screenshot` source "tab".
//
// Spliced into the reload script ahead of the status overlay, which draws
// the button that starts it. Published on `window.__idealyst_tab_capture`
// (`TAB_CAPTURE_GLOBAL` in lib.rs), where `backend-web`'s
// `robot_screenshot_source` calls `active()`, `request()` and `grab()`.
//
// Why a share stream: a page cannot read its own rendered pixels — only
// copy its DOM into an SVG (`dom`), which drops canvases drawn with WebGL,
// plugin content such as a PDF viewer, and exact font rendering. A tab the
// user shared is the browser's real output.
//
// Why a button: the browser opens `getDisplayMedia` only from a user's
// click, so an agent cannot start the stream; the user does, once, and the
// stream then serves every later screenshot. A full reload ends it (a share
// is not kept across page loads); in-place patches keep it.
//
// Scope: `preferCurrentTab` makes Chromium offer only THIS tab. Other
// browsers show their full picker, so the shared surface is checked:
// anything but a browser tab is refused, and where capture handles exist,
// a tab other than this one is too.
function idealystTabCapture(win, doc) {
  var stream = null, video = null;
  var requested = false;  // the agent asked while nothing was shared
  var lastShot = null;    // Date.now() of the last frame handed out
  var error = null;       // why the last start failed
  var listeners = [];
  // Set on this page so a capture handle can prove the shared tab is it.
  var handle = "idealyst-" + Math.random().toString(36).slice(2);

  function notify() {
    for (var i = 0; i < listeners.length; i++) {
      try { listeners[i](); } catch (_) {}
    }
  }

  function media() {
    return win.navigator && win.navigator.mediaDevices;
  }

  function supported() {
    var m = media();
    return !!(m && m.getDisplayMedia);
  }

  function active() {
    if (!stream) return false;
    var tracks = stream.getVideoTracks();
    for (var i = 0; i < tracks.length; i++) {
      if (tracks[i].readyState === "live") return true;
    }
    return false;
  }

  function release() {
    if (stream) stream.getTracks().forEach(function (t) { t.stop(); });
    stream = null;
    video = null;
  }

  function stop() {
    release();
    notify();
  }

  // Must run from the user's click (the browser's rule for getDisplayMedia).
  function start() {
    if (active()) return Promise.resolve();
    if (!supported()) {
      error = "this browser cannot share a tab";
      notify();
      return Promise.reject(new Error(error));
    }
    var m = media();
    try {
      if (m.setCaptureHandleConfig) m.setCaptureHandleConfig({ handle: handle, permittedOrigins: ["*"] });
    } catch (_) {}
    var dpr = win.devicePixelRatio || 1;
    return m.getDisplayMedia({
      video: {
        displaySurface: "browser",
        width: { ideal: Math.round(win.innerWidth * dpr) },
        height: { ideal: Math.round(win.innerHeight * dpr) },
        frameRate: { ideal: 30 }
      },
      audio: false,
      preferCurrentTab: true,
      selfBrowserSurface: "include",
      surfaceSwitching: "exclude",
      monitorTypeSurfaces: "exclude"
    }).then(function (s) {
      var track = s.getVideoTracks()[0];
      var surface = track && track.getSettings ? track.getSettings().displaySurface : undefined;
      var captured = track && track.getCaptureHandle ? track.getCaptureHandle() : null;
      var refusal = null;
      if (!track) refusal = "nothing was shared";
      else if (surface && surface !== "browser") refusal = "a " + surface + " was shared, not this tab";
      else if (captured && captured.handle !== handle) refusal = "another tab was shared, not this one";
      if (refusal) {
        s.getTracks().forEach(function (t) { t.stop(); });
        throw new Error(refusal);
      }
      track.addEventListener("ended", stop);
      var v = doc.createElement("video");
      v.muted = true;
      v.playsInline = true;
      v.srcObject = s;
      return v.play().then(function () {
        stream = s;
        video = v;
        requested = false;
        error = null;
        notify();
      });
    }).catch(function (e) {
      // Dismissing the browser's dialog is a choice, not a failure to show.
      error = e && e.name === "NotAllowedError" ? null : String((e && e.message) || e);
      notify();
      throw e;
    });
  }

  // Resolve once `n` new frames reached the video, or after `ms` —
  // a page that does not change sends no frames.
  function frames(n, ms) {
    return new Promise(function (resolve) {
      var done = false;
      var timer = win.setTimeout(finish, ms);
      function finish() {
        if (done) return;
        done = true;
        win.clearTimeout(timer);
        resolve();
      }
      function step(k) {
        if (done) return;
        if (k === 0 || !video.requestVideoFrameCallback) return finish();
        video.requestVideoFrameCallback(function () { step(k - 1); });
      }
      win.requestAnimationFrame(function () { step(n); });
    });
  }

  // One frame of the shared tab as {png_base64, width, height}. The dev
  // overlay is hidden for it, and two frames are awaited: the first can be
  // one composited before the hide (or before the robot's last action
  // painted).
  function grab() {
    if (!active()) return Promise.reject(new Error("the tab is not shared"));
    var host = doc.querySelector ? doc.querySelector("idealyst-dev-overlay") : null;
    var visibility = host ? host.style.visibility : "";
    if (host) host.style.visibility = "hidden";
    function restore() {
      if (host) host.style.visibility = visibility;
    }
    return frames(2, 500).then(function () {
      var w = video.videoWidth, h = video.videoHeight;
      if (!w || !h) throw new Error("the shared tab has sent no frame yet");
      var canvas = doc.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      canvas.getContext("2d").drawImage(video, 0, 0, w, h);
      var url = canvas.toDataURL("image/png");
      lastShot = Date.now();
      return { png_base64: url.slice(url.indexOf(",") + 1), width: w, height: h };
    }).then(function (frame) {
      restore();
      notify();
      return frame;
    }, function (e) {
      restore();
      throw e;
    });
  }

  // The agent asked for the tab while it was not shared: the overlay
  // shows the button lit until the user shares (or the page reloads).
  function request() {
    if (active() || requested) return;
    requested = true;
    notify();
  }

  function state() {
    return { supported: supported(), sharing: active(), requested: requested, lastShot: lastShot, error: error };
  }

  function subscribe(f) {
    listeners.push(f);
  }

  return { start: start, stop: stop, grab: grab, request: request, active: active, state: state, subscribe: subscribe };
}
