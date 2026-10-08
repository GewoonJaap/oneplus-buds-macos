// Thin client for buds-daemon. The Rust shell relays every daemon stdout line as a "line" event
// and forwards command lines (see src/bin/buds-daemon.rs) to its stdin. Battery history,
// statistics and notification decisions all come from the daemon (protocol v2).
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const send = (line) => invoke("send_cmd", { line });

const MODES = [["off", "Off"], ["transparency", "Transparency"], ["adaptive", "Adaptive"], ["nc", "Noise Cancellation"]];
const NC_LEVELS = [["high", "High"], ["medium", "Medium"], ["low", "Low"]];
const EQS = [["balanced", "Balanced"], ["bold", "Bold"], ["serenade", "Serenade"], ["bassboost", "Bass boost"], ["dynaudio", "Dynaudio"]];
const SPATIAL = [["off", "Off"], ["fixed", "Fixed"], ["headtracked", "Head Tracked"]];
const SIDES = [["left", "Left"], ["right", "Right"]];
const GESTURES = [["press", "Press"], ["double", "Press Twice"], ["triple", "Press 3 Times"], ["swipe", "Swipe"]];
const ACTIONS = {
  none: "None", playpause: "Play/Pause", previous: "Previous Track", next: "Next Track",
  voice: "Voice Assistant", gamemode: "Game Mode", volume: "Volume Control", skiptrack: "Skip Track",
};
const ACTIONS_FOR = {
  swipe: ["none", "volume", "skiptrack"],
  other: ["none", "playpause", "previous", "next", "voice", "gamemode"],
};
const HOLD = [[1, "Off"], [2, "Noise Cancellation"], [4, "Transparency"]];
const CELLS = { left: "Left earbud", right: "Right earbud", case: "Charging case" };

const prefs = Object.assign(
  { notifyLow: true, threshold: 20, notifyConn: false, autoCheck: true, autoInstall: false },
  safeParse(localStorage.getItem("prefs")),
);
function safeParse(s) { try { return JSON.parse(s) || {}; } catch { return {}; } }
function savePrefs() { try { localStorage.setItem("prefs", JSON.stringify(prefs)); } catch {} }

let state = null;
const group = (m) => (["high", "medium", "low"].includes(m) ? "nc" : m);

function button(text, on, click) {
  const b = document.createElement("button");
  b.textContent = text;
  if (on) b.classList.add("on");
  b.onclick = click;
  return b;
}

// ---------- navigation ----------
const PAGES = ["main", "controls", "stats", "settings"];
function show(page) {
  PAGES.forEach((p) => ($(p).hidden = p !== page));
  window.scrollTo(0, 0);
  if (page === "stats") requestStats();
  if (page === "controls") renderControls();
}
document.querySelectorAll(".back").forEach((b) => (b.onclick = () => show("main")));
$("open-settings").onclick = () => show("settings");
$("open-stats").onclick = () => show("stats");
$("open-controls").onclick = () => show("controls");

// ---------- main page ----------
function render() {
  const s = state || {};
  const connected = !!s.connected;
  document.body.classList.toggle("disconnected", !connected);
  $("status").textContent = t(connected ? "Connected" : "Not connected");
  $("reconnect").hidden = connected;

  const b = s.battery || {};
  $("batteries").replaceChildren(...Object.keys(CELLS).map((k) => {
    const c = b[k];
    const el = document.createElement("div");
    el.className = "cell";
    const ring = document.createElement("div");
    ring.className = "ring" + (c ? "" : " none") + (c && c.charging ? " charging" : "") + (c && !c.charging && c.percent <= prefs.threshold ? " low" : "");
    ring.style.setProperty("--p", c ? c.percent : 0);
    ring.innerHTML = `<span>${c ? c.percent + "%" : "–"}</span>`;
    const label = document.createElement("small");
    label.textContent = t(k === "case" ? "Case" : k === "left" ? "Left" : "Right") + (c && c.charging ? " ⚡" : "");
    el.append(ring, label);
    return el;
  }));

  const g = group(s.mode);
  $("modes").replaceChildren(...MODES.map(([k, label]) => button(t(label), g === k, () => {
    const m = k === "nc" ? "high" : k;
    send("mode " + m);
    state = { ...s, mode: m }; render();
  })));
  $("nc-levels").hidden = g !== "nc";
  $("nc-levels").replaceChildren(...NC_LEVELS.map(([k, label]) => button(t(label), s.mode === k, () => {
    send("mode " + k); state = { ...s, mode: k }; render();
  })));

  const eq = $("eq");
  if (!eq.options.length) EQS.forEach(([k, l]) => eq.add(new Option(t(l), k)));
  eq.value = s.eq || "";

  $("game").checked = !!s.game;
  $("spatial").replaceChildren(...SPATIAL.map(([k, l]) => button(t(l), s.spatial === k, () => {
    send("spatial " + k); state = { ...s, spatial: k }; render();
  })));

  if (!$("controls").hidden) renderControls();
}

$("eq").onchange = (e) => send("eq " + e.target.value);
$("game").onchange = (e) => send("game " + (e.target.checked ? "on" : "off"));
$("reconnect").onclick = () => send("reconnect");

// ---------- earbud controls ----------
let ctlSide = "left";
function renderControls() {
  const s = state || {};
  $("ctl-side").replaceChildren(...SIDES.map(([k, l]) => button(t(l), ctlSide === k, () => { ctlSide = k; renderControls(); })));
  const mine = (s.gestures || {})[ctlSide] || {};
  $("ctl-gestures").replaceChildren(...GESTURES.map(([gk, gl]) => {
    const row = document.createElement("div");
    row.className = "row";
    const label = document.createElement("span");
    label.textContent = t(gl);
    const sel = document.createElement("select");
    sel.className = "inline";
    for (const a of gk === "swipe" ? ACTIONS_FOR.swipe : ACTIONS_FOR.other) sel.add(new Option(t(ACTIONS[a]), a));
    sel.value = mine[gk] || "none";
    sel.onchange = () => {
      send(`gesture ${ctlSide} ${gk} ${sel.value}`);
      state = { ...s, gestures: { ...s.gestures, [ctlSide]: { ...mine, [gk]: sel.value } } };
    };
    row.append(label, sel);
    return row;
  }));
  const mask = s.hold ?? 0;
  $("ctl-hold").replaceChildren(...HOLD.map(([bit, l]) => button(t(l), mask & bit, () => {
    const next = mask ^ bit;
    // The buds need at least two modes in the cycle.
    if (bitCount(next) < 2) return;
    send("hold " + next);
    state = { ...s, hold: next }; renderControls();
  })));
}
const bitCount = (n) => n.toString(2).replace(/0/g, "").length;

// ---------- statistics ----------
let statsSide = "left";
let statsHours = 24;
let lastStats = null;

function requestStats() {
  $("stats-side").replaceChildren(...[...SIDES, ["case", "Case"]].map(([k, l]) => button(t(l), statsSide === k, () => { statsSide = k; requestStats(); })));
  $("stats-hours").replaceChildren(...[[24, "24 h"], [168, "7 days"]].map(([h, l]) => button(t(l), statsHours === h, () => { statsHours = h; requestStats(); })));
  send(`history ${statsSide} ${statsHours}`);
}

function duration(secs) {
  const m = Math.round(Math.max(secs, 60) / 60);
  const d = Math.floor(m / 1440), h = Math.floor((m % 1440) / 60), mm = m % 60;
  const parts = [];
  if (d) parts.push(d + "d");
  if (h) parts.push(h + "h");
  if (mm && parts.length < 2) parts.push(mm + "m");
  return parts.slice(0, 2).join(" ");
}

function renderStats(st) {
  lastStats = st;
  const NS = "http://www.w3.org/2000/svg";
  const W = 320, H = 170, padR = 30, padB = 18, top = 6;
  const plotW = W - padR, plotH = H - padB - top;
  const x = (tt) => ((tt - st.from) / (st.to - st.from)) * plotW;
  const y = (p) => top + plotH - (p / 100) * plotH;
  const el = (n, a, parent) => { const e = document.createElementNS(NS, n); for (const k in a) e.setAttribute(k, a[k]); parent.append(e); return e; };

  const chart = $("chart");
  if (!st.points.length) {
    chart.innerHTML = "";
    const p = document.createElement("p");
    p.className = "muted center";
    p.textContent = t("No battery history yet. Keep the app running to record it.");
    chart.append(p);
  } else {
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
    for (const v of [0, 50, 100]) {
      el("line", { x1: 0, x2: plotW, y1: y(v), y2: y(v), class: "grid" }, svg);
      const tx = el("text", { x: plotW + 4, y: y(v) + 3, class: "axis" }, svg); tx.textContent = v + "%";
    }
    for (const [a, b] of st.spans) {
      el("rect", { x: x(Math.max(a, st.from)), y: top, width: Math.max(2, x(Math.min(b, st.to)) - x(Math.max(a, st.from))), height: plotH, class: "span" }, svg);
    }
    const bw = Math.max(1.5, (st.bucket / (st.to - st.from)) * plotW * 0.7);
    for (const [tt, pct, ch] of st.points) {
      const low = !ch && pct <= prefs.threshold;
      el("rect", { x: x(tt) - bw / 2, y: y(pct), width: bw, height: Math.max(1, y(0) - y(pct)), class: low ? "bar low" : "bar" }, svg);
    }
    const fmt = new Intl.DateTimeFormat(document.documentElement.lang, st.hours === 24 ? { hour: "numeric" } : { weekday: "short" });
    for (let i = 0; i < 4; i++) {
      const tt = st.from + ((st.to - st.from) * (i + 0.5)) / 4;
      const tx = el("text", { x: x(tt), y: H - 4, class: "axis mid" }, svg); tx.textContent = fmt.format(new Date(tt * 1000));
    }
    chart.replaceChildren(svg);
  }

  const rows = [
    ["Average Drain", st.drain_per_hour != null ? st.drain_per_hour.toFixed(1) + " %/h" : "–"],
    ["Estimated Battery Life", st.est_life_secs != null ? duration(st.est_life_secs) : "–"],
    ["Time Charging", st.charge_secs >= 60 ? duration(st.charge_secs) : "–"],
    ["Average Level", st.average != null ? Math.round(st.average) + "%" : "–"],
  ];
  $("stats-rows").replaceChildren(...rows.map(([k, v]) => {
    const row = document.createElement("div");
    row.className = "row";
    const a = document.createElement("span"); a.textContent = t(k);
    const b = document.createElement("span"); b.className = "muted"; b.textContent = v;
    row.append(a, b);
    return row;
  }));
}

// ---------- daemon lines ----------
function onLine(line) {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.stats) {
    if (msg.stats.side === statsSide && msg.stats.hours === statsHours) renderStats(msg.stats);
  } else if (msg.event) {
    onEvent(msg);
  } else if ("connected" in msg) {
    state = msg;
    render();
  }
}

function onEvent(e) {
  if (e.event === "connected") invoke("notify", { title: "OnePlus Buds Pro 3", body: t("Buds connected") });
  else if (e.event === "disconnected") invoke("notify", { title: "OnePlus Buds Pro 3", body: t("Buds disconnected") });
  else if (e.event === "low_battery") {
    invoke("notify", { title: t("%@ battery low", t(CELLS[e.cell])), body: t("%ld%% remaining", e.percent) });
  }
}

function sendConfig() {
  send(`config ${prefs.notifyLow ? "on" : "off"} ${prefs.threshold} ${prefs.notifyConn ? "on" : "off"}`);
}

// ---------- settings ----------
$("quit").onclick = () => invoke("quit");
$("test-notify").onclick = () => invoke("notify", { title: "Buds", body: t("Notifications are working.") });
$("autostart").onchange = (e) => invoke("autostart_set", { enabled: e.target.checked });
$("notify-low").onchange = (e) => { prefs.notifyLow = e.target.checked; savePrefs(); sendConfig(); };
$("notify-conn").onchange = (e) => { prefs.notifyConn = e.target.checked; savePrefs(); sendConfig(); };
$("threshold").oninput = (e) => {
  prefs.threshold = +e.target.value; $("threshold-val").textContent = prefs.threshold;
  savePrefs(); sendConfig(); render();
};

// ---------- updates (Tauri updater plugin; see .github/workflows/release.yml) ----------
let isDev = false;
let pending = null;

async function checkUpdate(manual) {
  const st = $("update-status");
  if (manual) st.textContent = t("Checking…");
  try {
    pending = await invoke("check_update");
  } catch (e) {
    if (manual) st.textContent = t("Update failed");
    return;
  }
  const banner = $("update-banner");
  if (!pending) {
    banner.hidden = true;
    if (manual) st.textContent = t("You're up to date");
    return;
  }
  if (prefs.autoInstall && !isDev && !manual) return installUpdate();
  st.textContent = `${t("Update available")}: ${pending}`;
  banner.textContent = `${t("Update")} ${pending}`;
  banner.hidden = false;
}

async function installUpdate() {
  $("update-banner").textContent = t("Installing update…");
  $("update-banner").hidden = false;
  try { await invoke("install_update"); } catch (e) { $("update-status").textContent = t("Update failed"); }
}

$("update-banner").onclick = installUpdate;
$("check-now").onclick = () => checkUpdate(true);
$("auto-check").onchange = (e) => { prefs.autoCheck = e.target.checked; savePrefs(); };
$("auto-install").onchange = (e) => { prefs.autoInstall = e.target.checked; savePrefs(); };

// ---------- startup ----------
(async function init() {
  await loadLanguage();
  applyStatic();
  $("notify-low").checked = prefs.notifyLow;
  $("notify-conn").checked = prefs.notifyConn;
  $("threshold").value = prefs.threshold;
  $("threshold-val").textContent = prefs.threshold;
  $("auto-check").checked = prefs.autoCheck;
  $("auto-install").checked = prefs.autoInstall;
  invoke("autostart_get").then((v) => ($("autostart").checked = v));

  await listen("line", (e) => onLine(e.payload));
  sendConfig();
  const last = await invoke("get_state");
  if (last) onLine(last); else render();

  const [version, dev] = await invoke("app_info");
  isDev = dev;
  $("version").textContent = version;
  // Dev builds never check automatically (same as the macOS app); "Check Now" still works.
  if (prefs.autoCheck && !dev) {
    setTimeout(() => checkUpdate(false), 5000);
    setInterval(() => prefs.autoCheck && checkUpdate(false), 6 * 3600 * 1000);
  }
})();
