/* ThermalAI KernelSU WebUI
 * Talks to the daemon via `thermalair` CLI + direct file reads through ksu.exec().
 * Modern glassmorphism UI with fullscreen toggle (ksu.fullScreen) and
 * swipe navigation between tabs.
 */

const STATE_DIR = "/data/local/tmp/AIThermal/state";
const LOG_DIR = "/data/local/tmp/AIThermal";
const MODULE_DIR = "/data/adb/modules/thermalai_rust";
const TABS = ["dashboard", "policy", "gaming", "charging", "logs", "hardware"];
let activeTab = "dashboard";
let pageVisible = true;

/* ------------------------------------------------------------------ */
/* KernelSU bridge                                                    */
/* ------------------------------------------------------------------ */
function ksuExec(cmd) {
  return new Promise((resolve) => {
    if (typeof ksu === "undefined" || !ksu.exec) {
      // Browser fallback for development.
      resolve({ errno: 1, stdout: "", stderr: "ksu API unavailable (open inside KernelSU Manager)" });
      return;
    }
    const cbName = "__ksuCb_" + Math.random().toString(36).slice(2);
    window[cbName] = (errno, stdout, stderr) => {
      delete window[cbName];
      resolve({ errno: Number(errno), stdout: stdout || "", stderr: stderr || "" });
    };
    try {
      ksu.exec(cmd, "{}", cbName);
    } catch (e) {
      resolve({ errno: 1, stdout: "", stderr: String(e) });
    }
  });
}

function toast(msg) {
  const t = document.getElementById("toast");
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => t.classList.remove("show"), 2200);
  if (typeof ksu !== "undefined" && ksu.toast) {
    try { ksu.toast(msg); } catch {}
  }
}

async function readFile(path) {
  const r = await ksuExec(`cat "${path}" 2>/dev/null`);
  return r.errno === 0 ? r.stdout : "";
}

async function readJson(path) {
  const s = await readFile(path);
  if (!s.trim()) return null;
  try { return JSON.parse(s); } catch { return null; }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

/* ------------------------------------------------------------------ */
/* Tabs + swipe navigation                                            */
/* ------------------------------------------------------------------ */
function moveIndicator() {
  const active = document.querySelector(".tab.active");
  const ind = document.getElementById("tabIndicator");
  if (!active || !ind) return;
  ind.style.width = active.offsetWidth + "px";
  ind.style.transform = `translateX(${active.offsetLeft}px)`;
}

function switchTab(name, dir) {
  if (TABS.indexOf(name) === -1) return;
  activeTab = name;
  document.querySelectorAll(".tab").forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  document.querySelectorAll(".view").forEach((v) => {
    const on = v.id === "view-" + name;
    v.classList.remove("slide-left", "slide-right");
    v.classList.toggle("active", on);
    if (on && dir) v.classList.add(dir === "left" ? "slide-left" : "slide-right");
  });
  moveIndicator();
  loadTab(name);
}

document.querySelectorAll(".tab").forEach((btn) => {
  btn.addEventListener("click", () => switchTab(btn.dataset.tab, null));
});

/* Horizontal swipe between tabs. Vertical scrolling stays native
 * (body has touch-action: pan-y); swipes starting on interactive
 * elements (buttons, tabs) are ignored. */
const main = document.getElementById("main");
let sw = { x: 0, y: 0, on: false };

main.addEventListener("touchstart", (e) => {
  if (e.target.closest("button, a, .tab, .icon-btn")) { sw.on = false; return; }
  sw = { x: e.touches[0].clientX, y: e.touches[0].clientY, on: true };
}, { passive: true });

main.addEventListener("touchend", (e) => {
  if (!sw.on) return;
  sw.on = false;
  const dx = e.changedTouches[0].clientX - sw.x;
  const dy = e.changedTouches[0].clientY - sw.y;
  if (Math.abs(dx) < 64 || Math.abs(dx) < Math.abs(dy) * 1.25) return;
  const cur = TABS.indexOf(activeTab);
  const next = dx < 0 ? cur + 1 : cur - 1; // swipe left -> next tab
  if (next >= 0 && next < TABS.length) {
    switchTab(TABS[next], dx < 0 ? "left" : "right");
  }
}, { passive: true });

window.addEventListener("resize", moveIndicator);

/* ------------------------------------------------------------------ */
/* Fullscreen                                                         */
/* ------------------------------------------------------------------ */
let isFullscreen = false;

function applyFullscreen(on) {
  document.getElementById("fullscreenBtn").classList.toggle("on", on);
  if (typeof ksu === "undefined") return;
  try {
    if (typeof ksu.fullScreen === "function") {
      ksu.fullScreen(on);
    } else if (typeof ksu.setDisplayState === "function") {
      ksu.setDisplayState(on ? "open" : "close");
    }
  } catch (e) { /* manager may reject fullscreen; ignore */ }
}

document.getElementById("fullscreenBtn").addEventListener("click", () => {
  isFullscreen = !isFullscreen;
  applyFullscreen(isFullscreen);
});

/* ------------------------------------------------------------------ */
/* Dashboard                                                          */
/* ------------------------------------------------------------------ */
function tempColor(t) {
  if (t == null) return "var(--muted)";
  if (t >= 45) return "var(--danger)";
  if (t >= 40) return "var(--accent-2)";
  if (t >= 36) return "var(--warn)";
  return "var(--accent)";
}

function updateRing(t) {
  const ring = document.getElementById("tempRing");
  const min = 20, max = 55;
  const pct = Math.max(0, Math.min(1, (t - min) / (max - min)));
  const circ = 2 * Math.PI * 52;
  ring.style.strokeDasharray = circ.toFixed(1);
  ring.style.strokeDashoffset = (circ * (1 - pct)).toFixed(1);
  ring.style.stroke = tempColor(t);
}

function fmtDuration(ms) {
  if (!ms || ms < 0) return "—";
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return h ? `${h}h ${m}m` : m ? `${m}m ${sec}s` : `${sec}s`;
}

async function loadDashboard() {
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`);
  if (!state) {
    setDaemon(false);
    document.getElementById("tempValue").textContent = "–";
    return;
  }
  setDaemon(true);

  // Prefer battery temp; fall back to ai-adjusted temp for the gauge value.
  const temp = state.batt_temp ?? state.ai_temp ?? null;
  document.getElementById("tempValue").textContent = temp != null ? temp : "–";
  updateRing(temp ?? 0);

  const trend = state.trend_score;
  document.getElementById("tempTrend").textContent =
    trend != null ? `trend ${trend > 0 ? "▲" : trend < 0 ? "▼" : "→"} ${trend}` : "trend —";

  document.getElementById("policyValue").textContent = state.policy ?? "—";
  document.getElementById("gamingChip").textContent = "Gaming: " + (state.gaming ? "on" : "off");
  document.getElementById("cooldownChip").textContent = "Cooldown: " + (state.cooldown_active ? "active" : "off");
  const recoveryPhaseChip = document.getElementById("recoveryPhaseChip");
  if (state.recovery_phase && state.recovery_phase !== "None") {
    recoveryPhaseChip.style.display = "";
    recoveryPhaseChip.textContent = "Phase: " + state.recovery_phase;
  } else {
    recoveryPhaseChip.style.display = "none";
  }

  document.getElementById("gameValue").textContent = state.game_pkg || "—";
  document.getElementById("peakValue").textContent = (state.session_peak_temp ?? "—") + " °C";

  const startedEpoch = state.session_started_at;
  document.getElementById("durValue").textContent =
    startedEpoch ? fmtDuration(Date.now() - startedEpoch * 1000) : "—";

  document.getElementById("plugValue").textContent = state.plugged_in ? "yes" : "no";
  document.getElementById("screenValue").textContent = state.screen_off ? "yes" : "no";

  // Adaptive tier + GPU level are shown in the Session card only
  // when they are meaningful (i.e., not null and not a bare "—").
  const extraChips = [];
  if (state.adaptive_tier)   extraChips.push(`Tier: ${state.adaptive_tier}`);
  if (state.gpu_power_level != null) extraChips.push(`GPU lvl: ${state.gpu_power_level}`);
  const durEl = document.getElementById("durValue");
  if (extraChips.length) durEl.title = extraChips.join("  ·  ");

  document.getElementById("tickValue").textContent = (state.sleep_ms ?? "—") + " ms";
}

function setDaemon(running) {
  const pill = document.getElementById("daemonPill");
  pill.dataset.state = running ? "running" : "stopped";
  document.getElementById("daemonPillText").textContent = running ? "daemon running" : "daemon stopped";
}

async function loadZones() {
  const r = await ksuExec(`for z in /sys/class/thermal/thermal_zone*; do
    type=$(cat $z/type 2>/dev/null); t=$(cat $z/temp 2>/dev/null);
    [ -n "$t" ] && echo "$type|$t";
  done`);
  const zones = document.getElementById("zones");
  if (!r.stdout.trim()) { zones.innerHTML = '<div class="muted small">No zones available.</div>'; return; }
  const parsed = r.stdout.trim().split("\n").map((l) => {
    const [type, raw] = l.split("|");
    const c = Math.round(Number(raw) / (Math.abs(Number(raw)) > 1000 ? 1000 : 1));
    return { type: type || "?", c: Number.isFinite(c) ? c : 0 };
  }).filter((z) => z.c > 0).sort((a, b) => b.c - a.c);
  const maxT = Math.max(...parsed.map((z) => z.c), 1);
  zones.innerHTML = parsed.map((z) => {
    const cls = z.c >= 55 ? "hot" : z.c >= 45 ? "warm" : "";
    const bar = Math.max(6, Math.round((z.c / maxT) * 100));
    return `<div class="zone">
      <div class="zone-head">
        <div class="zone-name" title="${escapeHtml(z.type)}">${escapeHtml(z.type)}</div>
        <div class="zone-temp ${cls}">${z.c}°C</div>
      </div>
      <div class="zone-bar" style="width:${bar}%"></div>
    </div>`;
  }).join("");
}

/* ------------------------------------------------------------------ */
/* Tab loaders                                                        */
/* ------------------------------------------------------------------ */
async function loadPolicy() {
  const state = await readFile(`${STATE_DIR}/thermalai_state.json`);
  document.getElementById("policyRaw").textContent = state.trim() || "No state file.";
  const log = await readFile(`${LOG_DIR}/thermalai_thermal.log`);
  const lines = log.split("\n").filter((l) =>
    /transition|Policy changed|Applying policy|Evaluating policy|Starting session/.test(l)
  ).slice(-15);
  document.getElementById("historyRaw").textContent = lines.length ? lines.join("\n") : "No transitions logged yet.";
}

async function loadGaming() {
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`) || {};
  document.getElementById("gamingRaw").textContent = JSON.stringify({
    gaming: state.gaming,
    game: state.game_pkg,
    started_epoch: state.session_started_at,
    peak_temp: state.session_peak_temp,
    session_count: state.session_count,
    cooldown: state.cooldown_active,
    cooldown_source: state.cooldown_source_pkg,
  }, null, 2);
  // game_list.conf lives under the module's config/ directory (see main.rs).
  const list = await readFile(`${MODULE_DIR}/config/game_list.conf`);
  document.getElementById("gameListRaw").textContent = list.trim() || "game_list.conf not found.";
}

async function loadCharging() {
  const c = await readFile(`${STATE_DIR}/charging_session.json`);
  const mode = await readFile(`${STATE_DIR}/charging_mode.json`);
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`) || {};
  const header =
    `Active mode: ${state.charge_state ?? "—"}   Limit: ${state.charge_limit_ma ?? "—"} mA\n` +
    `Control node: ${state.charge_control_node ?? "(none — kernel/PMIC controls current)"}\n` +
    `Charge mode: ${state.charge_mode ?? "?"}\n` +
    `QCOM voters: ${state.qcom_voter_count ?? 0}\n` +
    `BatteryCare cap active: ${state.restrict_chg_active ? "yes" : "no"}\n` +
    (mode.trim() ? `Override: ${mode.trim()}\n\n` : "\n");
  document.getElementById("chargeRaw").textContent = header + (c.trim() || "No charging session recorded.");
}

const LOG_FILES = {
  logs:     "thermalai.log",
  thermal:  "thermalai_thermal.log",
  charging: "thermalai_charging.log",
  gaming:   "thermalai_gaming.log",
  battery:  "thermalai_battery.log",
  verbose:  "thermalai_verbose.log",
  ui:       "thermalai_ui.log",
};
let currentLog = "logs";
async function loadLogs(kind = currentLog) {
  currentLog = kind;
  const el = document.getElementById("logRaw");
  el.textContent = "Loading…";
  const name = LOG_FILES[kind] || LOG_FILES.logs;
  const r = await ksuExec(`tail -n 400 "${LOG_DIR}/${name}" 2>/dev/null`);
  el.textContent = r.stdout.trim() || "Log empty or missing.";
  el.scrollTop = el.scrollHeight;
}

async function loadHardware() {
  const cal = await readFile(`${STATE_DIR}/calibration.json`);
  document.getElementById("calRaw").textContent = cal.trim() || "No calibration state.";
}

function loadTab(name) {
  ({
    dashboard: () => { loadDashboard(); loadZones(); },
    policy: loadPolicy,
    gaming: loadGaming,
    charging: loadCharging,
    logs: () => loadLogs(),
    hardware: loadHardware,
  })[name]?.();
}

/* ------------------------------------------------------------------ */
/* Actions                                                            */
/* ------------------------------------------------------------------ */
async function daemonCmd(sub) {
  toast(`Running: thermalair ${sub}`);
  const r = await ksuExec(`thermalair ${sub}`);
  toast(r.errno === 0 ? `${sub} ok` : `${sub} failed`);
  loadDashboard();
}

document.getElementById("startBtn").onclick = () => daemonCmd("start");
document.getElementById("stopBtn").onclick = () => daemonCmd("stop");
document.getElementById("restartBtn").onclick = () => daemonCmd("restart");
document.getElementById("refreshTemps").onclick = loadZones;
document.getElementById("reloadGames").onclick = loadGaming;

document.querySelectorAll("[data-charge]").forEach((b) =>
  b.addEventListener("click", () => daemonCmd(`charging ${b.dataset.charge}`))
);
document.querySelectorAll("[data-log]").forEach((b) =>
  b.addEventListener("click", () => loadLogs(b.dataset.log))
);
document.getElementById("clearVerbose").onclick = async () => {
  await ksuExec(`thermalair verbose clear`);
  toast("Verbose log cleared");
  loadLogs("verbose");
};
document.getElementById("genReport").onclick = async () => {
  const el = document.getElementById("hwRaw");
  el.textContent = "Running thermalai-detect…";
  const r = await ksuExec(`thermalai-detect 2>&1 | tail -n 400`);
  el.textContent = r.stdout.trim() || r.stderr || "No output.";
};

/* ------------------------------------------------------------------ */
/* Version + boot                                                     */
/* ------------------------------------------------------------------ */
async function loadVersion() {
  const p = await readFile(`${MODULE_DIR}/module.prop`);
  const m = p.match(/version=([^\n]+)/);
  document.getElementById("brandSub").textContent = "Rust · " + (m ? m[1].trim() : "unknown");
}

loadVersion();
switchTab("dashboard", null);

/* Pause polling when the WebUI is not visible. */
if (typeof ksu !== "undefined") {
  try {
    if (typeof ksu.onWebUiVisible === "function") ksu.onWebUiVisible(() => { pageVisible = true; });
    if (typeof ksu.onWebUiHidden === "function") ksu.onWebUiHidden(() => { pageVisible = false; });
  } catch (e) {}
}
document.addEventListener("visibilitychange", () => { pageVisible = !document.hidden; });

/* Poll dashboard every 3s while it's the active tab */
setInterval(() => {
  if (!pageVisible) return;
  if (document.getElementById("view-dashboard").classList.contains("active")) {
    loadDashboard();
  }
  if (document.getElementById("view-logs").classList.contains("active")) {
    // gentle refresh
    loadLogs(currentLog);
  }
}, 3000);
