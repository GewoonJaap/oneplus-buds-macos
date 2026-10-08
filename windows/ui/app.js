// Thin client for buds-daemon. The Rust shell relays daemon JSON lines as "state" events and
// forwards command lines (see AGENTS.md / buds-daemon.rs parse_cmd) to its stdin.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const MODES = [
  { key: "off", label: "Off" },
  { key: "transparency", label: "Transparency" },
  { key: "adaptive", label: "Adaptive" },
  { key: "nc", label: "Noise Cancellation" },
];
const NC_LEVELS = [["high", "High"], ["medium", "Medium"], ["low", "Low"]];
const EQS = [["balanced", "Balanced"], ["bold", "Bold"], ["serenade", "Serenade"], ["bassboost", "Bass Boost"], ["dynaudio", "Dynaudio"]];
const SPATIAL = [["off", "Off"], ["fixed", "Fixed"], ["headtracked", "Head tracked"]];

const prefs = Object.assign(
  { notifyLow: true, threshold: 20, notifyConn: false },
  safeParse(localStorage.getItem("prefs")),
);
function safeParse(s) { try { return JSON.parse(s) || {}; } catch { return {}; } }
function savePrefs() { try { localStorage.setItem("prefs", JSON.stringify(prefs)); } catch {} }

let state = null;
let prev = null;
const lowAlerted = { left: false, right: false, case: false };

const send = (line) => invoke("send_cmd", { line });
const group = (m) => (["high", "medium", "low"].includes(m) ? "nc" : m);

function render() {
  const s = state || {};
  const connected = !!s.connected;
  document.body.classList.toggle("disconnected", !connected);
  $("status").textContent = connected ? "Connected" : "Not connected";
  $("reconnect").hidden = connected;

  const b = s.battery || {};
  $("batteries").replaceChildren(...[["left", "Left"], ["right", "Right"], ["case", "Case"]].map(([k, name]) => {
    const c = b[k];
    const el = document.createElement("div");
    el.className = "cell";
    const ring = document.createElement("div");
    ring.className = "ring" + (c ? "" : " none") + (c && c.charging ? " charging" : "") + (c && !c.charging && c.percent <= prefs.threshold ? " low" : "");
    ring.style.setProperty("--p", c ? c.percent : 0);
    ring.innerHTML = `<span>${c ? c.percent + "%" : "–"}</span>`;
    const label = document.createElement("small");
    label.textContent = name + (c && c.charging ? " ⚡" : "");
    el.append(ring, label);
    return el;
  }));

  const g = group(s.mode);
  $("modes").replaceChildren(...MODES.map((m) => button(m.label, g === m.key, () => {
    send("mode " + (m.key === "nc" ? "high" : m.key));
    state = { ...s, mode: m.key === "nc" ? "high" : m.key }; render();
  })));
  $("nc-levels").hidden = g !== "nc";
  $("nc-levels").replaceChildren(...NC_LEVELS.map(([k, label]) => button(label, s.mode === k, () => {
    send("mode " + k); state = { ...s, mode: k }; render();
  })));

  const eq = $("eq");
  if (!eq.options.length) EQS.forEach(([k, l]) => eq.add(new Option(l, k)));
  eq.value = s.eq || "";

  $("game").checked = !!s.game;
  $("spatial").replaceChildren(...SPATIAL.map(([k, l]) => button(l, s.spatial === k, () => {
    send("spatial " + k); state = { ...s, spatial: k }; render();
  })));
}

function button(text, on, click) {
  const b = document.createElement("button");
  b.textContent = text;
  if (on) b.classList.add("on");
  b.onclick = click;
  return b;
}

// Notification decisions live here for now; they move into the daemon in the shared-backend refactor.
function evaluate() {
  if (!state) return;
  if (prev && prefs.notifyConn && prev.connected !== state.connected) {
    invoke("notify", { title: "OnePlus Buds Pro 3", body: state.connected ? "Connected" : "Disconnected" });
  }
  const b = state.battery || {};
  for (const k of ["left", "right", "case"]) {
    const c = b[k];
    if (!c) continue;
    if (c.charging || c.percent > prefs.threshold + 5) lowAlerted[k] = false;
    else if (prefs.notifyLow && !lowAlerted[k] && c.percent <= prefs.threshold) {
      lowAlerted[k] = true;
      invoke("notify", { title: "Battery low", body: `${k[0].toUpperCase() + k.slice(1)}: ${c.percent}%` });
    }
  }
}

function onState(line) {
  try { prev = state; state = JSON.parse(line); } catch { return; }
  evaluate();
  render();
}

$("eq").onchange = (e) => send("eq " + e.target.value);
$("game").onchange = (e) => send("game " + (e.target.checked ? "on" : "off"));
$("reconnect").onclick = () => send("reconnect");
$("open-settings").onclick = () => { $("main").hidden = true; $("settings").hidden = false; };
$("back").onclick = () => { $("settings").hidden = true; $("main").hidden = false; };
$("quit").onclick = () => invoke("quit");
$("test-notify").onclick = () => invoke("notify", { title: "Buds", body: "Notifications are working." });
$("autostart").onchange = (e) => invoke("autostart_set", { enabled: e.target.checked });
$("notify-low").onchange = (e) => { prefs.notifyLow = e.target.checked; savePrefs(); };
$("notify-conn").onchange = (e) => { prefs.notifyConn = e.target.checked; savePrefs(); };
$("threshold").oninput = (e) => { prefs.threshold = +e.target.value; $("threshold-val").textContent = prefs.threshold; savePrefs(); render(); };

$("notify-low").checked = prefs.notifyLow;
$("notify-conn").checked = prefs.notifyConn;
$("threshold").value = prefs.threshold;
$("threshold-val").textContent = prefs.threshold;
invoke("autostart_get").then((v) => ($("autostart").checked = v));

listen("state", (e) => onState(e.payload));
invoke("get_state").then((l) => { if (l) onState(l); else render(); });
