/// Builds the strict CSP-compliant vanilla JavaScript runtime for the `https://miku.local`
/// Overview scene. Uses `textContent` and `createElement`/`createElementNS` exclusively.
pub(crate) fn build_overview_script() -> String {
    r###"(function() {
  "use strict";
  var ORIGIN = "https://miku.local/index.html";
  var MOOD_META = {
    idle:        { glow: "#BDDEFF", eyes: "open",  mouth: "idle",  blush: false, brows: false },
    listening:   { glow: "#BDDEFF", eyes: "open",  mouth: "talk",  blush: false, brows: false },
    thinking:    { glow: "#C7E0FF", eyes: "open",  mouth: "idle",  blush: false, brows: false },
    working:     { glow: "#BDDEFF", eyes: "open",  mouth: "idle",  blush: false, brows: true  },
    talking:     { glow: "#3B9EFF", eyes: "open",  mouth: "talk",  blush: false, brows: false },
    happy:       { glow: "#FFE5B4", eyes: "arc",   mouth: "smile", blush: true,  brows: false },
    focused:     { glow: "#3B9EFF", eyes: "open",  mouth: "idle",  blush: false, brows: true  },
    love:        { glow: "#FFD2E8", eyes: "arc",   mouth: "smile", blush: true,  brows: false },
    wink:        { glow: "#BDDEFF", eyes: "wink",  mouth: "smile", blush: true,  brows: false },
    surprised:   { glow: "#E2D8FF", eyes: "open",  mouth: "talk",  blush: true,  brows: true  },
    annoyed:     { glow: "#FFC9C6", eyes: "open",  mouth: "frown", blush: true,  brows: true  },
    sleepy:      { glow: "#BDDEFF", eyes: "blink", mouth: "idle",  blush: false, brows: false },
    sleeping:    { glow: "#BDDEFF", eyes: "blink", mouth: "idle",  blush: false, brows: false },
    error:       { glow: "#FFD2CF", eyes: "x",     mouth: "frown", blush: false, brows: true  },
    alert:       { glow: "#FF453A", eyes: "x",     mouth: "frown", blush: false, brows: true  }
  };
  var seq = 1;

  function applySafeText(id, text) {
    var el = document.getElementById(id);
    if (el) { el.textContent = String(text); }
  }

  function showOnlyGroup(prefix, active, keys) {
    for (var i = 0; i < keys.length; i++) {
      var node = document.getElementById(prefix + keys[i]);
      if (node) { node.style.display = keys[i] === active ? "inline" : "none"; }
    }
  }

  function setMood(moodName) {
    var key = MOOD_META[moodName] ? moodName : "idle";
    var cfg = MOOD_META[key];
    document.body.setAttribute("data-mood", key);
    applySafeText("miku-avatar-mood-label", key);
    var flood = document.getElementById("miku-glow-flood");
    if (flood) { flood.setAttribute("flood-color", cfg.glow); }
    var blush = document.getElementById("miku-layer-blush");
    if (blush) { blush.style.display = cfg.blush ? "inline" : "none"; blush.setAttribute("data-visible", String(cfg.blush)); }
    var brows = document.getElementById("miku-layer-brows");
    if (brows) { brows.style.display = cfg.brows ? "inline" : "none"; brows.setAttribute("data-visible", String(cfg.brows)); }
    showOnlyGroup("miku-eyes-", cfg.eyes, ["open", "blink", "arc", "x", "wink"]);
    showOnlyGroup("miku-mouth-", cfg.mouth, ["idle", "talk", "smile", "frown"]);
    var sel = document.getElementById("miku-mood-select");
    if (sel && sel.value !== key) { sel.value = key; }
  }

  function postIpc(eventKind, targetId) {
    var frame = {
      jsonrpc: "2.0",
      v: 1,
      id: "ui-" + (seq++),
      origin: ORIGIN,
      payload: {
        kind: "ui_interaction",
        data: { event: eventKind, target_id: String(targetId), x: 0, y: 0, timestamp_ms: Date.now() }
      }
    };
    if (window.chrome && window.chrome.webview && window.chrome.webview.postMessage) {
      window.chrome.webview.postMessage(JSON.stringify(frame));
    }
  }

  function appendFeedItem(listId, text, maxItems) {
    var list = document.getElementById(listId);
    if (!list) { return; }
    var li = document.createElement("li");
    li.className = "miku-feed-item";
    li.textContent = String(text);
    list.insertBefore(li, list.firstChild);
    while (list.children.length > maxItems) { list.removeChild(list.lastChild); }
  }

  function renderQr(seedStr) {
    var svg = document.getElementById("miku-link-qr");
    if (!svg) { return; }
    while (svg.firstChild) { svg.removeChild(svg.firstChild); }
    var ns = svg.namespaceURI;
    var bg = document.createElementNS(ns, "rect");
    bg.setAttribute("x", "0"); bg.setAttribute("y", "0"); bg.setAttribute("width", "21"); bg.setAttribute("height", "21");
    bg.setAttribute("fill", "#FFFFFF"); bg.setAttribute("rx", "2");
    svg.appendChild(bg);
    var hash = 2166136261;
    for (var k = 0; k < seedStr.length; k++) { hash = (hash ^ seedStr.charCodeAt(k)) * 16777619 >>> 0; }
    for (var y = 1; y < 20; y++) {
      for (var x = 1; x < 20; x++) {
        var inFinder = (x < 6 && y < 6) || (x > 14 && y < 6) || (x < 6 && y > 14);
        var on = inFinder ? ((x === 1 || x === 5 || y === 1 || y === 5 || x === 15 || x === 19 || y === 15 || y === 19 || (x >= 2 && x <= 4 && y >= 2 && y <= 4) || (x >= 16 && x <= 18 && y >= 2 && y <= 4) || (x >= 2 && x <= 4 && y >= 16 && y <= 18))) : (((hash + x * 31 + y * 17) % 3) === 0);
        if (on) {
          var r = document.createElementNS(ns, "rect");
          r.setAttribute("x", String(x)); r.setAttribute("y", String(y));
          r.setAttribute("width", "1"); r.setAttribute("height", "1"); r.setAttribute("fill", "#0E0F11");
          svg.appendChild(r);
        }
      }
    }
  }

  function switchTab(tabName) {
    var tabs = document.querySelectorAll(".miku-tab");
    for (var i = 0; i < tabs.length; i++) {
      var active = tabs[i].getAttribute("data-tab") === tabName;
      tabs[i].setAttribute("aria-selected", active ? "true" : "false");
    }
    var panels = document.querySelectorAll(".miku-tabpanel");
    for (var j = 0; j < panels.length; j++) {
      var match = panels[j].id === "panel-" + tabName;
      if (match) { panels[j].classList.add("is-active"); } else { panels[j].classList.remove("is-active"); }
    }
  }

  document.addEventListener("click", function(ev) {
    var tab = ev.target.closest ? ev.target.closest("[data-tab]") : null;
    if (tab) { switchTab(tab.getAttribute("data-tab")); postIpc("action_click", "tab:" + tab.getAttribute("data-tab")); return; }
    var btn = ev.target.closest ? ev.target.closest("[data-action]") : null;
    if (!btn) { return; }
    var act = btn.getAttribute("data-action");
    if (act === "perm_allow") { applySafeText("miku-overview-status", "Permiso Allow Once firmado"); setMood("happy"); }
    else if (act === "perm_deny") { applySafeText("miku-overview-status", "Permiso denegado por Gatekeeper"); setMood("annoyed"); }
    else if (act === "prompt_send") {
      var inp = document.getElementById("miku-prompt-input");
      var txt = inp && inp.value ? inp.value.trim() : "status check";
      appendFeedItem("miku-chat-queue", "[Prompt] " + txt, 12);
      if (inp) { inp.value = ""; }
      setMood("thinking");
    } else if (act === "ptt_toggle") {
      applySafeText("miku-media-ducking", "Ducking 20%: Listening"); setMood("listening");
    } else if (act === "model_select") {
      var mdl = btn.getAttribute("data-model") || "claude-sonnet";
      applySafeText("miku-active-model-badge", mdl);
      applySafeText("miku-overview-status", "Modelo activo: " + mdl);
    } else if (act === "skill_reload") {
      applySafeText("miku-skill-status", "skill.toml recargado (0 leaks)");
    } else if (act === "link_pair") {
      var token = "miku-link-" + (seq++);
      renderQr(token);
      applySafeText("miku-link-tailscale-status", "QR rotado (" + token + ") via Tailscale");
    } else if (act === "kill_switch") {
      applySafeText("miku-overview-status", "KILL SWITCH ACTIVO: audio/red/tunnels cortados");
      setMood("alert");
    }
    postIpc("action_click", act);
  });

  var moodSel = document.getElementById("miku-mood-select");
  if (moodSel) {
    moodSel.addEventListener("change", function() {
      setMood(moodSel.value);
      postIpc("action_click", "mood:" + moodSel.value);
    });
  }

  var drop = document.getElementById("miku-file-dropzone");
  if (drop) {
    drop.addEventListener("dragover", function(e) { e.preventDefault(); });
    drop.addEventListener("drop", function(e) {
      e.preventDefault();
      var name = (e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0]) ? e.dataTransfer.files[0].name : "contexto.txt";
      applySafeText("miku-file-drop-label", "Adjuntado seguro: " + name);
      postIpc("action_click", "file_drop");
    });
  }

  document.addEventListener("keydown", function(ev) {
    if (ev.key === "Escape") { postIpc("escape_pressed", "island-root"); }
  });

  if (window.chrome && window.chrome.webview) {
    window.chrome.webview.addEventListener("message", function(ev) {
      try {
        var msg = typeof ev.data === "string" ? JSON.parse(ev.data) : ev.data;
        if (!msg || !msg.payload) { return; }
        if (msg.payload.kind === "dom_safe_text_render") {
          applySafeText(msg.payload.data.element_id, msg.payload.data.safe_text);
          appendFeedItem("miku-overview-feed", msg.payload.data.safe_text, 24);
        } else if (msg.payload.kind === "telemetry_snapshot") {
          var d = msg.payload.data;
          applySafeText("miku-overview-ram-badge", "RAM " + (d.ram_private_mb_x100 / 100).toFixed(2) + " MB");
          applySafeText("miku-overview-ipc-badge", "IPC RTT " + d.rtt_us + " us");
        }
      } catch (_err) {}
    });
  }

  setMood("idle");
  renderQr("miku-link-v1-ready");
})();"###.to_string()
}
