/* ThermalAI KernelSU WebUI
 * Talks to the daemon via `thermalair` CLI + direct file reads through ksu.exec().
 */

const STATE_DIR = "/data/local/tmp/thermalai_state";
const LOG_DIR = "/data/local/tmp";
const MODULE_DIR = "/data/adb/modules/thermalai_rust";

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

/* ------------------------------------------------------------------ */
/* Tabs                                                               */
/* ------------------------------------------------------------------ */
document.querySelectorAll(".tab").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((b) => b.classList.remove("active"));
    document.querySelectorAll(".view").forEach((v) => v.classList.remove("active"));
    btn.classList.add("active");
    document.getElementById("view-" + btn.dataset.tab).classList.add("active");
    loadTab(btn.dataset.tab);
  });
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
  const rows = r.stdout.trim().split("\n").map((l) => {
    const [type, raw] = l.split("|");
    const c = Math.round(Number(raw) / (Math.abs(Number(raw)) > 1000 ? 1000 : 1));
    const cls = c >= 55 ? "hot" : c >= 45 ? "warm" : "";
    return `<div class="zone"><div class="zone-name">${type || "?"}</div><div class="zone-temp ${cls}">${c}°C</div></div>`;
  }).join("");
  zones.innerHTML = rows;
}

/* ------------------------------------------------------------------ */
/* Tab loaders                                                        */
/* ------------------------------------------------------------------ */
async function loadPolicy() {
  const state = await readFile(`${STATE_DIR}/thermalai_state.json`);
  document.getElementById("policyRaw").textContent = state.trim() || "No state file.";
  const log = await readFile(`${LOG_DIR}/thermalai.log`);
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
loadDashboard();
loadZones();

/* Poll dashboard every 3s while it's the active tab */
setInterval(() => {
  if (document.getElementById("view-dashboard").classList.contains("active")) {
    loadDashboard();
  }
  if (document.getElementById("view-logs").classList.contains("active")) {
    // gentle refresh
    loadLogs(currentLog);
  }
}, 3000);
