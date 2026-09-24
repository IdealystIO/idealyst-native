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
//   that applied the save, and how long it took);
// - a panel over the page when a build fails: each rustc error with its
//   file:line and rendered message. Esc or a click outside the message
//   dismisses it; the next successful build or patch clears it.
//
// DOM use is deliberately narrow (createElement, appendChild, textContent,
// style.cssText, setAttribute, addEventListener, attachShadow) so the
// behaviour is testable in node against a small fake DOM
// (`crates/dev/http/tests/status_overlay.rs`). The shadow root keeps the
// app's CSS out of the overlay and the overlay's out of the app.
function idealystStatusOverlay(doc) {
  var FONT = "font:12px/1.4 ui-monospace,SFMono-Regular,Menlo,monospace;";
  var host = null, badge = null, panel = null, list = null, title = null;
  var st = {
    phase: "idle",      // idle | change | patching | building | done | error
    text: "",
    stage: null,
    progress: null,     // {compiled, total, current}
    pending: [],        // error diagnostics of the build in flight
    failure: null,      // {target, error, diagnostics} of the last failed build
    dismissed: false
  };

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
    badge = el("div",
      "position:fixed;right:8px;bottom:8px;z-index:2147483647;max-width:60vw;" +
      "padding:3px 8px;border-radius:4px;background:#111c;color:#eee;" +
      "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;pointer-events:none;" + FONT);
    badge.setAttribute("data-part", "badge");
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
    root.appendChild(badge);
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
    }
    render();
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
    badge.textContent = t;
    badge.style.color = st.phase === "error" ? "#ff8b8b" : st.phase === "done" ? "#8be28b" : "#eee";

    var show = !!st.failure && !st.dismissed;
    panel.style.display = show ? "block" : "none";
    if (!show) return;
    var f = st.failure;
    title.textContent = "Build failed" + (f.target ? " (" + f.target + ")" : "");
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

  return { apply: apply, dismiss: dismiss, state: st };
}
