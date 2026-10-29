// The in-page build-state overlay for `idealyst dev`.
//
// Spliced into the reload script (`RELOAD_SCRIPT` in lib.rs), which feeds
// it every `dev-state` event from the reload stream: the same versioned
// objects `idealyst dev --events-file` writes, projected to what
// describes the build (`dev_events::page::wants`).
//
// Plain JS and CSS on purpose: the overlay has to work when the page's
// wasm is the thing that failed to build — a Rust error must still reach
// the screen. A richer overlay can be an idealyst component once the
// bundle is live; this one owes nothing to it.
//
// It shows two things:
// - a small badge in the corner: what the dev loop is doing (building,
//   with cargo's progress and the current stage) or last did (the tier
//   that applied the save, and how long it took). In a full-stack session
//   the project's own server is tracked on its own: "building server…"
//   (with its progress) while it builds, "restarting server" until it
//   answers again — this page is served by it, and a reload waits for it;
// - a panel over the page when a build fails: each rustc error with its
//   file:line and rendered message. Esc or a click outside the message
//   dismisses it; the next successful build or patch clears it. A
//   full-stack server that stopped (its port taken, or it exited) shows
//   the same panel with what it said;
// - above the badge, this page's own Robot connection, once the app's
//   robot client reports one (`robot(status)`, called through
//   `window.__idealyst_dev_robot_status` by `backend-web`'s
//   `robot_transport`): connecting, connected, or what failed and when it
//   retries. The page reports it itself because the relay cannot: a page
//   that never reaches the relay is invisible to it;
// - on that robot line, while the robot is connected, the "allow capture"
//   chip of the tab capture (`capture`, from `tab_capture.js`): the click a
//   browser requires before it asks the user for permission to capture
//   the tab, after which the Robot's `screenshot` source "tab" is the
//   browser's real pixels. It lights up when an agent asked for it, and
//   while capturing it says so and stops the capture on a click.
//
// The "–" on the status line collapses all of that to one status glyph
// (remembered for the page in localStorage); the glyph expands it again
// and lights up when an agent asks for capture. A failed build's panel
// still opens over the page while collapsed — an error is never hidden.
//
// In a browser under automation (`navigator.webdriver`: Playwright,
// Selenium, any WebDriver/CDP-driven run) the corner shows nothing at
// all, not even the glyph: its buttons take clicks, so whatever the app
// draws under the bottom-right corner (the last row's menu) could not be
// clicked by the test. Nobody is there to read it, and nobody can
// answer the capture prompt the chip starts. A failed build's panel
// still opens.
//
// DOM use is deliberately narrow (createElement, appendChild, textContent,
// style.cssText, setAttribute, addEventListener, attachShadow) so the
// behaviour is testable in node against a small fake DOM
// (`crates/dev/http/tests/status_overlay.rs`). The shadow root keeps the
// app's CSS out of the overlay and the overlay's out of the app.
function idealystStatusOverlay(doc, capture) {
  var FONT = "font:12px/1.4 ui-monospace,SFMono-Regular,Menlo,monospace;";
  var host = null, stack = null, badge = null, robotRow = null, robotBadge = null, shareButton = null,
    collapseButton = null, pill = null, panel = null, list = null, title = null;
  var COLLAPSED_KEY = "idealyst-dev-overlay-collapsed";
  // Per-page convenience only: storage can be missing or throw (private
  // windows, blocked site data), and the overlay must render without it.
  function storage() {
    try {
      var w = doc.defaultView;
      return w && w.localStorage ? w.localStorage : null;
    } catch (_) {
      return null;
    }
  }
  var automated = false;
  try {
    var nav = doc.defaultView && doc.defaultView.navigator;
    automated = !!(nav && nav.webdriver);
  } catch (_) {}
  var collapsed = false;
  try {
    var s = storage();
    collapsed = !!s && s.getItem(COLLAPSED_KEY) === "1";
  } catch (_) {}
  function setCollapsed(v) {
    collapsed = v;
    try {
      var s = storage();
      if (s) s.setItem(COLLAPSED_KEY, v ? "1" : "0");
    } catch (_) {}
    render();
  }
  // This page's Robot connection, as the app's robot client last reported
  // it ({state, url, error, retry_ms}); null until it reports.
  var robot = null;
  var st = {
    phase: "idle",      // idle | change | patching | building | done | error
    text: "",
    stage: null,
    progress: null,     // {compiled, total, current}
    pending: [],        // error diagnostics of the build in flight
    failure: null,      // {target, error, diagnostics} of the last failed build
    dismissed: false
  };
  // The full-stack server, whose events carry its own target: kept apart
  // so its build and the bundle's can both be shown at once.
  var serverTarget = "server";
  var srv = { phase: "idle", initial: false, stage: null, progress: null, pending: [] };

  function applyServer(ev) {
    switch (ev.type) {
      case "change_detected":
        srv.phase = "change";
        break;
      case "build_started":
        srv.phase = "building";
        srv.initial = ev.cause === "initial";
        srv.stage = null;
        srv.progress = null;
        srv.pending = [];
        break;
      case "stage_started":
        srv.stage = ev.stage;
        break;
      case "cargo_progress":
        srv.progress = { compiled: ev.compiled, total: ev.total, current: ev.current };
        break;
      case "diagnostic":
        if (ev.diagnostic && srv.pending.length < 64) srv.pending.push(ev.diagnostic);
        break;
      case "server_down":
        failServer(ev);
        break;
      case "build_finished":
        if (ev.outcome === "failed") {
          // The old server keeps running; the error is shown like any build's.
          srv.phase = "idle";
          st.phase = "error";
          st.text = "server build failed";
          st.failure = { target: serverTarget, error: ev.error, diagnostics: srv.pending };
          st.dismissed = false;
        } else if (ev.outcome === "ready") {
          srv.phase = "starting";
        } else if (ev.outcome === "unchanged") {
          srv.phase = "idle";
        } else {
          srv.phase = "restarting";
        }
        srv.pending = [];
        srv.stage = null;
        srv.progress = null;
        break;
    }
  }

  function serverBusy() {
    return srv.phase !== "idle";
  }

  function serverText() {
    var t;
    if (srv.phase === "change") t = "… server change";
    else if (srv.phase === "starting") t = "⚙ starting server";
    else if (srv.phase === "restarting") t = "⚙ restarting server";
    else {
      t = "⚙ " + (srv.initial ? "building" : "rebuilding") + " server…";
      if (srv.progress) {
        t += " " + srv.progress.compiled + (srv.progress.total ? "/" + srv.progress.total : "");
        if (srv.progress.current) t += " " + srv.progress.current;
      }
      if (srv.stage) t += " · " + srv.stage;
    }
    return t;
  }

  function el(tag, css) {
    var e = doc.createElement(tag);
    if (css) e.style.cssText = css;
    return e;
  }

  function mount() {
    if (host) return;
    host = el("idealyst-dev-overlay", "all:initial;");
    host.setAttribute("data-idealyst-dev-overlay", "");
    var root = host.attachShadow ? host.attachShadow({ mode: "open" }) : host;
    // One bottom-right column (robot line above the status line), so
    // lines that wrap push each other up instead of overlapping. The
    // column ignores the pointer; only its buttons take clicks.
    stack = el("div",
      "position:fixed;right:8px;bottom:8px;z-index:2147483647;max-width:60vw;" +
      "display:flex;flex-direction:column;align-items:flex-end;gap:4px;pointer-events:none;");
    stack.setAttribute("data-part", "stack");
    var ROW = "display:flex;align-items:center;gap:6px;max-width:100%;padding:3px 8px;" +
      "border-radius:4px;background:#111c;color:#eee;box-sizing:border-box;" + FONT;
    var CHIP = "padding:0 6px;border-radius:3px;border:1px solid #fff3;background:transparent;" +
      "color:#eee;cursor:pointer;pointer-events:auto;white-space:nowrap;" + FONT;
    robotRow = el("div", ROW + "display:none;");
    robotRow.setAttribute("data-part", "robot-row");
    robotBadge = el("span", "white-space:normal;overflow-wrap:anywhere;");
    robotBadge.setAttribute("data-part", "robot");
    shareButton = el("button", CHIP + "display:none;");
    shareButton.setAttribute("data-part", "share");
    shareButton.setAttribute("type", "button");
    shareButton.addEventListener("click", function () {
      if (!capture) return;
      if (capture.state().sharing) capture.stop();
      // The browser's permission prompt follows; its outcome comes back
      // through `subscribe`. A dismissed prompt is no error to show.
      else capture.start().catch(function () {});
    });
    robotRow.appendChild(robotBadge);
    robotRow.appendChild(shareButton);
    var statusRow = el("div", ROW);
    badge = el("span", "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0;");
    badge.setAttribute("data-part", "badge");
    collapseButton = el("button", CHIP + "border:none;padding:0 2px;color:#999;");
    collapseButton.setAttribute("data-part", "collapse");
    collapseButton.setAttribute("type", "button");
    collapseButton.setAttribute("title", "Hide the dev overlay");
    collapseButton.textContent = "–";
    collapseButton.addEventListener("click", function () { setCollapsed(true); });
    statusRow.appendChild(badge);
    statusRow.appendChild(collapseButton);
    stack.appendChild(robotRow);
    stack.appendChild(statusRow);
    pill = el("button",
      "position:fixed;right:8px;bottom:8px;z-index:2147483647;display:none;" +
      "width:22px;height:22px;padding:0;border-radius:11px;border:1px solid #fff3;" +
      "background:#111c;color:#eee;cursor:pointer;" + FONT);
    pill.setAttribute("data-part", "pill");
    pill.setAttribute("type", "button");
    pill.setAttribute("title", "Show the dev overlay");
    pill.addEventListener("click", function () { setCollapsed(false); });
    panel = el("div",
      "position:fixed;inset:0;z-index:2147483647;background:#000b;display:none;" +
      "overflow:auto;padding:5vh 5vw;box-sizing:border-box;" + FONT);
    panel.setAttribute("data-part", "panel");
    var card = el("div",
      "background:#1b1b1f;color:#eee;border-top:3px solid #e5484d;padding:12px 16px;" +
      "max-width:1100px;margin:0 auto;");
    card.setAttribute("data-part", "card");
    title = el("div", "color:#ff8b8b;font-weight:bold;margin-bottom:8px;");
    list = el("div", "");
    var hint = el("div", "color:#999;margin-top:10px;");
    hint.textContent = "Esc or click outside to dismiss. Fix the error and save: this clears on the next successful build.";
    card.appendChild(title);
    card.appendChild(list);
    card.appendChild(hint);
    panel.appendChild(card);
    root.appendChild(stack);
    root.appendChild(pill);
    root.appendChild(panel);
    (doc.body || doc.documentElement).appendChild(host);
    panel.addEventListener("click", function (e) {
      if (e.target === panel) dismiss();
    });
    doc.addEventListener("keydown", function (e) {
      if (e.key === "Escape") dismiss();
    });
  }

  function dismiss() {
    if (!st.failure) return;
    st.dismissed = true;
    render();
  }

  function base(p) {
    var s = String(p || "");
    var i = Math.max(s.lastIndexOf("/"), s.lastIndexOf("\\"));
    return i >= 0 ? s.slice(i + 1) : s;
  }

  function ms(n) {
    return n >= 1000 ? (n / 1000).toFixed(1) + " s" : n + " ms";
  }

  function succeeded(text) {
    st.phase = "done";
    st.text = text;
    st.stage = null;
    st.progress = null;
    st.failure = null;
    st.dismissed = false;
  }

  function apply(ev) {
    if (ev.type === "session_started" && ev.server && ev.server.target) {
      serverTarget = ev.server.target;
    }
    if (ev.target === serverTarget) {
      applyServer(ev);
      render();
      return;
    }
    if (ev.type === "server_ready" && ev.kind === "full_stack") {
      // The server answers again. The page's own reload (if one was
      // waiting on it) follows on the reload stream.
      var restarted = srv.phase === "restarting";
      srv.phase = "idle";
      if (restarted && st.phase !== "building" && st.phase !== "patching" && st.phase !== "change") {
        succeeded("server restarted");
      }
      render();
      return;
    }
    switch (ev.type) {
      case "change_detected": {
        var n = (ev.paths || []).length;
        st.phase = "change";
        st.text = "change: " + base((ev.paths || [])[0]) + (n > 1 ? " (+" + (n - 1) + ")" : "");
        break;
      }
      case "decided": {
        var d = ev.decision || {};
        if (d.tier === "overlay") { st.phase = "patching"; st.text = "overlay patch"; }
        else if (d.tier === "hot_patch") { st.phase = "patching"; st.text = "hot patch: " + (d.crates || []).join(", "); }
        else if (d.tier === "rebuild") { st.phase = "building"; st.text = "rebuild"; }
        else if (d.tier === "unchanged") {
          // The source is back to what is running: an error from a build
          // of some other source no longer describes it.
          succeeded("no change");
          st.phase = "idle";
        }
        break;
      }
      case "build_started":
        st.phase = "building";
        st.text = ev.cause === "initial" ? "building" : "rebuilding";
        st.stage = null;
        st.progress = null;
        st.pending = [];
        break;
      case "stage_started":
        st.stage = ev.stage;
        break;
      case "cargo_progress":
        st.progress = { compiled: ev.compiled, total: ev.total, current: ev.current };
        break;
      case "diagnostic":
        if (ev.diagnostic && st.pending.length < 64) st.pending.push(ev.diagnostic);
        break;
      case "build_finished": {
        if (ev.outcome === "failed") {
          st.phase = "error";
          st.text = "build failed";
          st.stage = null;
          st.progress = null;
          st.failure = { target: ev.target, error: ev.error, diagnostics: st.pending };
          st.dismissed = false;
        } else if (ev.outcome === "reloaded" || ev.outcome === "premint_refreshed") {
          succeeded("rebuilt in " + ms(ev.ms) + ", reloading");
        } else if (ev.outcome === "unchanged") {
          succeeded("rebuilt, nothing changed");
        } else {
          succeeded("ready");
        }
        st.pending = [];
        break;
      }
      case "patch_built":
        succeeded("hot patch · " + ev.redirected + " fn · " + ms(ev.ms));
        break;
      case "patch_failed":
        st.text = "no patch (" + ev.reason + "), rebuilding";
        break;
      case "superseded":
        // The restart's own change_detected follows at once.
        st.text = "newer save, restarting";
        break;
      case "overlay_pushed":
        succeeded("overlay · " + ev.sites + " site(s) · " + ms(ev.ms));
        break;
      case "sidecar_applied":
        succeeded((ev.how === "hot_patch" ? "hot patch" : "respawn") + " · " + ms(ev.ms));
        break;
      case "error":
        st.phase = "error";
        st.text = ev.source + ": " + ev.message;
        break;
      // Normally filed under the server's target (handled above); a
      // stopped server is an error whatever it is filed under.
      case "server_down":
        failServer(ev);
        break;
    }
    render();
  }

  function serverDownText(ev) {
    if (ev.reason === "port_in_use") {
      return "port " + ev.port + " is already in use" + (ev.holder ? " by " + ev.holder : "");
    }
    return "exited with " + ev.status + (ev.cause ? ": " + ev.cause : "");
  }

  function failServer(ev) {
    srv.phase = "idle";
    var what = serverDownText(ev);
    st.phase = "error";
    st.text = "server " + (ev.reason === "port_in_use" ? "not started" : "stopped") + ": " + what;
    st.stage = null;
    st.progress = null;
    st.failure = {
      target: ev.target,
      title: ev.reason === "port_in_use" ? "Server not started" : "Server stopped",
      error: what + (ev.log_file ? "\n\nIts output is in " + ev.log_file : ""),
      diagnostics: []
    };
    st.dismissed = false;
  }

  // The robot client's report. Accepts the JSON string the client
  // publishes, or an object.
  function setRobot(status) {
    try {
      robot = typeof status === "string" ? JSON.parse(status) : status;
    } catch (_) {
      return;
    }
    render();
  }

  function robotText() {
    switch (robot.state) {
      case "connected": return { text: "● robot connected", color: "#8be28b" };
      case "connecting": return { text: "◌ robot connecting…", color: "#999" };
      case "dropped":
        return { text: "◌ robot disconnected (" + robot.error + "), reconnecting…", color: "#e5b443" };
      case "retrying":
        return {
          text: "✗ " + robot.error + " · retrying in " + ms(robot.retry_ms || 0),
          color: "#ff8b8b"
        };
      default: return { text: "✗ robot: " + (robot.error || robot.state), color: "#ff8b8b" };
    }
  }

  function clock(at) {
    var d = new Date(at);
    function two(n) { return (n < 10 ? "0" : "") + n; }
    return two(d.getHours()) + ":" + two(d.getMinutes()) + ":" + two(d.getSeconds());
  }

  // What the capture chip says, or null when it is not shown: only while
  // the robot is connected (no agent can ask otherwise) and the browser
  // can capture a tab — or once an agent asked, so a browser that cannot
  // still says why. The click asks the BROWSER for permission to capture
  // this tab (`getDisplayMedia`); `title` carries the longer explanation.
  function shareText() {
    if (!capture || !robot || robot.state !== "connected") return null;
    var c = capture.state();
    if (!c.supported) {
      return c.requested
        ? { text: "✗ capture unsupported", title: "An agent asked to capture this tab, but this browser cannot capture a tab", color: "#ff8b8b", lit: false }
        : null;
    }
    if (c.sharing) {
      return {
        text: "◉ capturing · stop",
        title: "The agent's screenshots of this tab are the browser's real pixels" +
          (c.lastShot ? " · last screenshot " + clock(c.lastShot) : "") + ". Click to stop.",
        color: "#8be28b",
        lit: false
      };
    }
    var t = c.requested ? "agent wants capture · allow" : "allow capture";
    if (c.error) t += " (" + c.error + ")";
    return {
      text: t,
      title: "Lets the agent screenshot this tab with the browser's real pixels. The browser asks for permission.",
      color: c.requested ? "#111" : "#eee",
      lit: c.requested
    };
  }

  var ICON = { idle: "○", change: "…", patching: "↻", building: "⚙", done: "✓", error: "✗" };

  function render() {
    mount();
    var t = ICON[st.phase] + " " + st.text;
    if (st.phase === "building") {
      if (st.progress) {
        t += " " + st.progress.compiled + (st.progress.total ? "/" + st.progress.total : "");
        if (st.progress.current) t += " " + st.progress.current;
      }
      if (st.stage) t += " · " + st.stage;
    }
    if (serverBusy()) {
      // The server's state leads: while it builds or restarts, it is what
      // the page is waiting on.
      t = st.phase === "idle" || st.phase === "done" ? serverText() : serverText() + " · " + t;
    }
    var statusColor = serverBusy() ? "#eee"
      : st.phase === "error" ? "#ff8b8b" : st.phase === "done" ? "#8be28b" : "#eee";
    badge.textContent = t;
    badge.style.color = statusColor;
    robotRow.style.display = robot ? "flex" : "none";
    if (robot) {
      var r = robotText();
      robotBadge.textContent = r.text;
      robotBadge.style.color = r.color;
    }
    var share = shareText();
    shareButton.style.display = share ? "inline-block" : "none";
    if (share) {
      shareButton.textContent = share.text;
      shareButton.setAttribute("title", share.title);
      shareButton.style.color = share.color;
      shareButton.style.background = share.lit ? "#e5b443" : "transparent";
    }
    stack.style.display = collapsed || automated ? "none" : "flex";
    pill.style.display = collapsed && !automated ? "block" : "none";
    pill.textContent = ICON[serverBusy() ? "building" : st.phase];
    pill.style.color = share && share.lit ? "#111" : statusColor;
    // An agent's capture request is the one thing worth a collapsed
    // overlay's attention short of an error (which opens the panel).
    pill.style.background = share && share.lit ? "#e5b443" : "#111c";

    var show = !!st.failure && !st.dismissed;
    panel.style.display = show ? "block" : "none";
    if (!show) return;
    var f = st.failure;
    title.textContent = (f.title || "Build failed") + (f.target ? " (" + f.target + ")" : "");
    while (list.firstChild) list.removeChild(list.firstChild);
    var diags = f.diagnostics || [];
    if (!diags.length) {
      var pre = el("pre", "white-space:pre-wrap;margin:0;");
      pre.textContent = f.error || "";
      list.appendChild(pre);
    }
    for (var i = 0; i < diags.length; i++) {
      var d = diags[i];
      var item = el("div", "margin:0 0 12px;");
      item.setAttribute("data-part", "diagnostic");
      var where = el("div", "color:#8fb8ff;margin-bottom:4px;");
      var loc = d.file ? d.file + (d.line ? ":" + d.line + (d.column ? ":" + d.column : "") : "") : "";
      where.textContent = loc ? loc + " — " + d.message : d.message;
      var body = el("pre", "white-space:pre-wrap;margin:0;color:#ddd;");
      body.textContent = d.rendered || d.message;
      item.appendChild(where);
      item.appendChild(body);
      list.appendChild(item);
    }
  }

  if (capture) capture.subscribe(render);

  return { apply: apply, dismiss: dismiss, robot: setRobot, state: st };
}
