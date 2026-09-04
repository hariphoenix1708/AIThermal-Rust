/* ThermalAI WebUI — Modern Edition */
const STATE_DIR = "/data/local/tmp/AIThermal/state";
const LOG_DIR = "/data/local/tmp/AIThermal";
const MODULE_DIR = "/data/adb/modules/thermalai_rust";
const CONFIG_PATH = MODULE_DIR + "/config/profiles.conf";
const GAMELIST_PATH = MODULE_DIR + "/config/game_list.conf";
const TABS = ["dashboard", "gaming", "charging", "logs", "settings"];
let activeTab = "dashboard";
let pageVisible = true;
let lastState = null;

/* KernelSU bridge */
function ksuExec(cmd) {
  return new Promise((resolve) => {
    if (typeof ksu === "undefined" || !ksu.exec) { resolve({ errno: 1, stdout: "", stderr: "ksu unavailable" }); return; }
    const cb = "__cb_" + Math.random().toString(36).slice(2);
    window[cb] = (e, o, s) => { delete window[cb]; resolve({ errno: Number(e), stdout: o || "", stderr: s || "" }); };
    try { ksu.exec(cmd, "{}", cb); } catch (e) { resolve({ errno: 1, stdout: "", stderr: String(e) }); }
  });
}
function toast(msg) {
  const t = document.getElementById("toast"); t.textContent = msg; t.classList.add("show");
  clearTimeout(toast._t); toast._t = setTimeout(() => t.classList.remove("show"), 2200);
  if (typeof ksu !== "undefined" && ksu.toast) try { ksu.toast(msg); } catch {}
}
async function readFile(p) { const r = await ksuExec(`cat "${p}" 2>/dev/null`); return r.errno === 0 ? r.stdout : ""; }
async function readJson(p) { const s = await readFile(p); if (!s.trim()) return null; try { return JSON.parse(s); } catch { return null; } }
function escapeHtml(s) { return String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }

/* Navigation */
function switchTab(name) {
  if (!TABS.includes(name)) return;
  activeTab = name;
  document.querySelectorAll(".nav-item").forEach(b => b.classList.toggle("active", b.dataset.tab === name));
  document.querySelectorAll(".view").forEach(v => v.classList.toggle("active", v.id === "view-" + name));
  loadTab(name);
}
document.querySelectorAll(".nav-item").forEach(b => b.addEventListener("click", () => switchTab(b.dataset.tab)));

/* Notifications */
function checkCriticalEvents(state) {
  if (!state || !lastState) { lastState = state; return; }
  if (state.policy === "EmergencyCool" && lastState.policy !== "EmergencyCool")
    showNotification("🔥 EMERGENCY COOLING", "SoC critically hot — max throttle", "danger");
  if (state.recovery_mode && !lastState.recovery_mode)
    showNotification("⚠️ Recovery Mode", "Watchdog stall — restoring safe state", "warn");
  if (state.gaming && !lastState.gaming) {
    const p = (state.game_pkg || "?").split(".").pop();
    showNotification("🎮 Game Detected", `GameTurbo activating for ${p}`, "accent");
  }
  if (!state.gaming && lastState.gaming)
    showNotification("🎮 Session Ended", `Peak: ${lastState.session_peak_temp || "?"}°C`, "accent");
  if (state.cooldown_active && !lastState.cooldown_active)
    showNotification("❄️ Cooldown", "Post-game thermal cooldown", "muted");
  lastState = state;
}
function showNotification(title, body, sev) {
  const c = document.getElementById("notificationContainer"); if (!c) return;
  const colors = { danger: "var(--danger)", warn: "var(--warn)", accent: "var(--accent)", muted: "var(--muted)" };
  const el = document.createElement("div"); el.className = "notification";
  el.style.borderLeftColor = colors[sev] || "var(--text)";
  el.innerHTML = `<div class="notif-title" style="color:${colors[sev]}">${escapeHtml(title)}</div><div class="notif-body">${escapeHtml(body)}</div>`;
  c.appendChild(el);
  setTimeout(() => { el.style.opacity = "0"; el.style.transform = "translateX(100%)"; setTimeout(() => el.remove(), 300); }, 5000);
  if (typeof ksu !== "undefined" && ksu.toast) try { ksu.toast(title); } catch {}
}

/* Dashboard */
function tempColor(t) { if (t == null) return "var(--muted)"; if (t >= 55) return "var(--danger)"; if (t >= 45) return "var(--accent-2)"; if (t >= 36) return "var(--warn)"; return "var(--accent)"; }
function updateRing(t) {
  const r = document.getElementById("tempRing"); if (!r) return;
  const pct = Math.max(0, Math.min(1, (t - 20) / 35)); const c = 2 * Math.PI * 52;
  r.style.strokeDasharray = c.toFixed(1); r.style.strokeDashoffset = (c * (1 - pct)).toFixed(1); r.style.stroke = tempColor(t);
}
function fmtDuration(ms) { if (!ms || ms < 0) return "—"; const s = Math.floor(ms / 1000), m = Math.floor(s / 60), sec = s % 60; return m ? `${m}m ${sec}s` : `${sec}s`; }
async function loadDashboard() {
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`);
  if (!state) { setDaemon(false); document.getElementById("tempValue").textContent = "–"; return; }
  setDaemon(true);
  const temp = state.batt_temp ?? state.ai_temp ?? null;
  document.getElementById("tempValue").textContent = temp != null ? temp : "–"; updateRing(temp ?? 0);
  const trend = state.trend_score;
  document.getElementById("tempTrend").textContent = trend != null ? `trend ${trend > 0 ? "▲" : trend < 0 ? "▼" : "→"} ${trend}` : "trend —";
  document.getElementById("policyValue").textContent = state.policy ?? "—";
  document.getElementById("gamingChip").textContent = "Gaming: " + (state.gaming ? "on" : "off");
  document.getElementById("cooldownChip").textContent = "Cooldown: " + (state.cooldown_active ? "active" : "off");
  document.getElementById("gameValue").textContent = state.game_pkg || "—";
  document.getElementById("peakValue").textContent = (state.session_peak_temp ?? "—") + " °C";
  const started = state.session_started_at;
  document.getElementById("durValue").textContent = started ? fmtDuration(Date.now() - started * 1000) : "—";
  document.getElementById("plugValue").textContent = state.plugged_in ? "yes" : "no";
  document.getElementById("screenValue").textContent = state.screen_off ? "off" : "on";
  document.getElementById("tickValue").textContent = (state.sleep_ms ?? "—") + " ms";
}
function setDaemon(r) { const p = document.getElementById("daemonPill"); p.dataset.state = r ? "running" : "stopped"; document.getElementById("daemonPillText").textContent = r ? "running" : "stopped"; }
async function loadZones() {
  const r = await ksuExec(`for z in /sys/class/thermal/thermal_zone*; do type=$(cat $z/type 2>/dev/null); t=$(cat $z/temp 2>/dev/null); [ -n "$t" ] && echo "$type|$t"; done`);
  const el = document.getElementById("zones"); if (!r.stdout.trim()) { el.innerHTML = '<div class="muted small">No zones.</div>'; return; }
  const zones = r.stdout.trim().split("\n").map(l => { const [t, raw] = l.split("|"); const c = Math.round(Number(raw) / (Math.abs(Number(raw)) > 1000 ? 1000 : 1)); return { type: t || "?", c: Number.isFinite(c) ? c : 0 }; }).filter(z => z.c > 0).sort((a, b) => b.c - a.c);
  const max = Math.max(...zones.map(z => z.c), 1);
  el.innerHTML = zones.map(z => { const cls = z.c >= 55 ? "hot" : z.c >= 45 ? "warm" : ""; const w = Math.max(6, Math.round((z.c / max) * 100)); return `<div class="zone"><div class="zone-name">${escapeHtml(z.type)}</div><div class="zone-temp ${cls}">${z.c}°C</div><div class="zone-bar" style="width:${w}%"></div></div>`; }).join("");
}

/* Gaming */
async function loadLiveGaming() {
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`);
  const el = document.getElementById("liveGamingContent");
  if (!state || !state.gaming) { el.innerHTML = '<div class="muted small">No active game</div>'; return; }
  const pkg = (state.game_pkg || "?").split(".").pop();
  const temp = state.ai_temp ?? "–", policy = state.policy ?? "–", gpu = state.gpu_load ?? 0;
  const tier = state.adaptive_tier ?? "–", gpuLvl = state.gpu_power_level ?? "–", peak = state.session_peak_temp ?? "–";
  const fP50 = state.frame_p50_us != null ? (state.frame_p50_us / 1000).toFixed(1) + "ms" : "n/a";
  const fP90 = state.frame_p90_us != null ? (state.frame_p90_us / 1000).toFixed(1) + "ms" : "n/a";
  const fMax = state.frame_max_consecutive_jank ?? "–";
  let dur = "–"; if (state.session_started_at) { const e = Date.now() / 1000 - state.session_started_at; dur = Math.floor(e / 60) + "m " + Math.floor(e % 60) + "s"; }
  const tc = t => t >= 55 ? "var(--danger)" : t >= 48 ? "var(--accent-2)" : t >= 40 ? "var(--warn)" : "var(--accent)";
  const gb = Math.min(100, gpu);
  let cd = ""; if (state.cooldown_active) cd = `<div class="cooldown-banner">⏱ Cooldown active</div>`;
  el.innerHTML = `
    <div class="live-row"><div class="live-metric"><div class="live-label">Game</div><div class="live-value">${escapeHtml(pkg)}</div></div><div class="live-metric"><div class="live-label">Duration</div><div class="live-value">${dur}</div></div><div class="live-metric"><div class="live-label">Policy</div><div class="live-value" style="color:${tc(temp)}">${policy}</div></div><div class="live-metric"><div class="live-label">Tier</div><div class="live-value">${tier}</div></div></div>
    <div class="live-row"><div class="live-metric"><div class="live-label">Temp</div><div class="live-value" style="color:${tc(temp)}">${temp}°C</div></div><div class="live-metric"><div class="live-label">Peak</div><div class="live-value">${peak}°C</div></div><div class="live-metric"><div class="live-label">GPU</div><div class="live-value"><div class="mini-bar"><div class="mini-bar-fill" style="width:${gb}%;background:${gb>80?'var(--danger)':gb>50?'var(--accent-2)':'var(--accent)'}"></div></div>${gpu}%</div></div><div class="live-metric"><div class="live-label">GPU Lvl</div><div class="live-value">${gpuLvl}</div></div></div>
    <div class="live-row"><div class="live-metric"><div class="live-label">p50</div><div class="live-value">${fP50}</div></div><div class="live-metric"><div class="live-label">p90</div><div class="live-value">${fP90}</div></div><div class="live-metric"><div class="live-label">Max Jank</div><div class="live-value">${fMax}</div></div></div>
    ${cd}
    <div class="live-row"><div class="live-metric" style="flex:3"><div class="live-label">Temp Sparkline</div><div id="tempSparkline" class="sparkline-container"></div></div></div>`;
  renderSparkline();
}
async function renderSparkline() {
  const r = await ksuExec(`tail -30 "${LOG_DIR}/thermalai_thermal.log" 2>/dev/null`);
  if (!r.stdout.trim()) return;
  const temps = r.stdout.trim().split("\n").map(l => { const m = l.match(/composite=(\d+)C/); return m ? parseInt(m[1]) : null; }).filter(t => t !== null);
  if (temps.length < 2) return;
  const el = document.getElementById("tempSparkline"); if (!el) return;
  const w = 280, h = 36, pad = 2, min = Math.min(...temps) - 1, max = Math.max(...temps) + 1, range = max - min || 1;
  const pts = temps.map((t, i) => `${(pad + (i / (temps.length - 1)) * (w - 2 * pad)).toFixed(1)},${(h - pad - ((t - min) / range) * (h - 2 * pad)).toFixed(1)}`).join(" ");
  const last = temps[temps.length - 1], color = last >= 55 ? "var(--danger)" : last >= 48 ? "var(--accent-2)" : last >= 40 ? "var(--warn)" : "var(--accent)";
  el.innerHTML = `<svg viewBox="0 0 ${w} ${h}" width="100%" height="${h}"><polyline points="${pts}" fill="none" stroke="${color}" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><circle cx="${w - pad}" cy="${(h - pad - ((last - min) / range) * (h - 2 * pad)).toFixed(1)}" r="3" fill="${color}"/></svg>`;
}
async function loadGaming() {
  loadLiveGaming();
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`) || {};
  document.getElementById("gamingRaw").textContent = JSON.stringify({ gaming: state.gaming, game: state.game_pkg, peak: state.session_peak_temp, turbo: state.game_turbo_active, gpu: state.gpu_power_level, tier: state.adaptive_tier }, null, 2);
  const combat = await readFile(`${LOG_DIR}/thermalai_combat.log`);
  document.getElementById("combatRaw").textContent = combat.trim().split("\n").slice(-30).join("\n") || "No combat events.";
  const sess = await readFile(`${STATE_DIR}/game_session_profiles.json`);
  const turbo = await readFile(`${STATE_DIR}/game_turbo_profiles.json`);
  let p = ""; if (sess.trim()) p += "── session ──\n" + sess.trim().slice(0, 600) + "\n\n"; if (turbo.trim()) p += "── turbo ──\n" + turbo.trim().slice(0, 600);
  document.getElementById("profilesRaw").textContent = p.trim() || "No profiles yet.";
}

/* Charging */
async function loadCharging() {
  const c = await readFile(`${STATE_DIR}/charging_session.json`);
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`) || {};
  const hdr = `Mode: ${state.charge_mode ?? "—"}  Limit: ${state.charge_limit_ma ?? "—"} mA\nNode: ${state.charge_control_node ?? "(none)"}\nQCOM voters: ${state.qcom_voter_count ?? 0}\n`;
  document.getElementById("chargeRaw").textContent = hdr + (c.trim() || "No session recorded.");
}

/* Logs */
const LOG_FILES = { logs: "thermalai.log", thermal: "thermalai_thermal.log", gaming: "thermalai_gaming.log", battery: "thermalai_battery.log", charging: "thermalai_charging.log", ui: "thermalai_ui.log", combat: "thermalai_combat.log", verbose: "thermalai_verbose.log" };
let currentLog = "logs";
async function loadLogs(k) {
  const switched = k !== undefined && k !== currentLog;
  if (k !== undefined) currentLog = k;
  const el = document.getElementById("logRaw");
  const nameEl = document.getElementById("logFileName");
  const metaEl = document.getElementById("logMeta");
  document.querySelectorAll("[data-log]").forEach(b => b.classList.toggle("active", b.dataset.log === currentLog));
  const fname = LOG_FILES[currentLog] || LOG_FILES.logs;
  if (nameEl) nameEl.textContent = fname;
  if (switched) { el.classList.add("switching"); if (metaEl) metaEl.textContent = "Loading\u2026"; }
  const r = await ksuExec(`tail -n 400 "${LOG_DIR}/${fname}" 2>/dev/null`);
  const body = r.stdout.trim();
  el.textContent = body || "Log empty.";
  el.scrollTop = el.scrollHeight;
  if (metaEl) metaEl.textContent = `${body ? body.split("\n").length : 0} lines \u00b7 newest 400`;
  if (switched) {
    const tab = document.querySelector(`[data-log="${currentLog}"]`);
    if (tab && typeof tab.scrollIntoView === "function") tab.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "center" });
    requestAnimationFrame(() => requestAnimationFrame(() => el.classList.remove("switching")));
  }
}

/* Settings */
const CONFIG_FIELDS = ["temp_cool", "temp_warm", "temp_hot", "temp_powersave", "temp_critical", "poll_interval", "game_poll_interval", "policy_debounce_sec", "policy_debounce_gaming_sec"];
const TOGGLE_FIELDS = ["adaptive_governor_enabled", "game_turbo_enabled", "advanced_tuning_enabled", "ml_shadow_enabled", "network_diagnostics_enabled", "disable_tweaks"];
async function loadSettings() {
  const raw = await readFile(CONFIG_PATH);
  document.getElementById("gameListEditor").value = (await readFile(GAMELIST_PATH)).trim();
  if (!raw.trim()) return;
  CONFIG_FIELDS.forEach(f => { const el = document.getElementById("cfg_" + f); if (el) { const m = raw.match(new RegExp(f + '\\s*=\\s*(\\d+)')); if (m) el.value = m[1]; } });
  TOGGLE_FIELDS.forEach(f => { const el = document.getElementById("cfg_" + f); if (el) { const m = raw.match(new RegExp(f + '\\s*=\\s*(\\w+)')); if (m) el.checked = m[1] === "true"; } });
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`);
  document.getElementById("policyRaw").textContent = state ? JSON.stringify(state, null, 2) : "No state.";
  const soc = await ksuExec(`echo "WALT upmigrate: $(cat /proc/sys/kernel/sched_upmigrate 2>/dev/null || cat /proc/sys/kernel/sched_walt_upmigrate 2>/dev/null)"; echo "DDR min: $(cat /sys/class/devfreq/soc:qcom,cpubw/min_freq 2>/dev/null | head -c 20)"; echo "Uclamp FG: $(cat /dev/cpuctl/foreground/cpu.uclamp.min 2>/dev/null)"`);
  document.getElementById("socRaw").textContent = soc.stdout.trim() || "No SoC data.";
}
async function saveConfig() {
  let raw = await readFile(CONFIG_PATH); if (!raw.trim()) { toast("Config not found"); return; }
  CONFIG_FIELDS.forEach(f => { const el = document.getElementById("cfg_" + f); if (el && el.value) raw = raw.replace(new RegExp(`(${f}\\s*=\\s*)\\d+`), `$1${el.value}`); });
  TOGGLE_FIELDS.forEach(f => { const el = document.getElementById("cfg_" + f); if (el) raw = raw.replace(new RegExp(`(${f}\\s*=\\s*)(true|false)`), `$1${el.checked}`); });
  await ksuExec(`cat > "${CONFIG_PATH}" << 'CONFEOF'\n${raw}\nCONFEOF`);
  await ksuExec("thermalair restart");
  toast("Config saved & daemon restarted");
}
async function saveGameList() {
  const list = document.getElementById("gameListEditor").value.trim();
  await ksuExec(`cat > "${GAMELIST_PATH}" << 'GLEOF'\n${list}\nGLEOF`);
  // No CLI call needed: the daemon's file watcher hot-reloads game_list.conf
  // via on_config_reload within ~1 tick (there is no `thermalair reload-games` command).
  toast("Game list saved (hot-reloads automatically)");
}
async function loadHardware() {
  const cal = await readFile(`${STATE_DIR}/calibration.json`);
  document.getElementById("hwRaw").textContent = cal.trim() || "No calibration data.";
}

/* Tab router */
function loadTab(n) {
  ({ dashboard: () => { loadDashboard(); loadZones(); }, gaming: loadGaming, charging: loadCharging, logs: () => loadLogs(), settings: loadSettings })[n]?.();
}

/* Actions */
async function daemonCmd(sub) { toast(`thermalair ${sub}`); const r = await ksuExec(`thermalair ${sub}`); toast(r.errno === 0 ? `${sub} ok` : `${sub} failed`); loadDashboard(); }
document.getElementById("startBtn").onclick = () => daemonCmd("start");
document.getElementById("stopBtn").onclick = () => daemonCmd("stop");
document.getElementById("restartBtn").onclick = () => daemonCmd("restart");
document.getElementById("refreshTemps").onclick = loadZones;
document.getElementById("reloadCombat")?.addEventListener("click", loadGaming);
document.getElementById("reloadProfiles")?.addEventListener("click", loadGaming);
document.getElementById("saveConfigBtn")?.addEventListener("click", saveConfig);
document.getElementById("saveGameList")?.addEventListener("click", saveGameList);
document.getElementById("genReport")?.addEventListener("click", async () => { document.getElementById("hwRaw").textContent = "Running…"; const r = await ksuExec("thermalai-detect 2>&1 | tail -n 300"); document.getElementById("hwRaw").textContent = r.stdout.trim() || "No output."; });
document.getElementById("applyRecFeatures")?.addEventListener("click", () => {
  const rec = { adaptive_governor_enabled: true, game_turbo_enabled: true, advanced_tuning_enabled: true, ml_shadow_enabled: true, network_diagnostics_enabled: true, disable_tweaks: false };
  Object.entries(rec).forEach(([k, v]) => { const el = document.getElementById("cfg_" + k); if (el) el.checked = v; });
  toast("Recommended toggles applied \u2014 tap Save Config & Reload");
});
document.getElementById("clearVerbose")?.addEventListener("click", async () => { await ksuExec("thermalair verbose clear"); toast("Verbose cleared"); loadLogs("verbose"); });
document.querySelectorAll("[data-charge]").forEach(b => b.addEventListener("click", () => daemonCmd(`charging ${b.dataset.charge}`)));
document.querySelectorAll("[data-log]").forEach(b => b.addEventListener("click", () => loadLogs(b.dataset.log)));

/* Version */
async function loadVersion() { const p = await readFile(`${MODULE_DIR}/module.prop`); const m = p.match(/version=([^\n]+)/); document.getElementById("brandSub").textContent = "Rust · " + (m ? m[1].trim() : "?"); }

/* Fullscreen */
document.getElementById("fullscreenBtn").addEventListener("click", () => {
  if (typeof ksu === "undefined") return;
  try { if (typeof ksu.fullScreen === "function") ksu.fullScreen(true); } catch {}
});

/* Poll */
setInterval(async () => {
  if (!pageVisible) return;
  const state = await readJson(`${STATE_DIR}/thermalai_state.json`);
  if (state) checkCriticalEvents(state);
  if (activeTab === "dashboard") { loadDashboard(); loadZones(); }
  if (activeTab === "gaming") loadLiveGaming();
  if (activeTab === "logs") loadLogs();
}, 3000);

/* Visibility */
if (typeof ksu !== "undefined") {
  try { if (ksu.onWebUiVisible) ksu.onWebUiVisible(() => { pageVisible = true; }); if (ksu.onWebUiHidden) ksu.onWebUiHidden(() => { pageVisible = false; }); } catch {}
}
document.addEventListener("visibilitychange", () => { pageVisible = !document.hidden; });

/* Init */
loadVersion();
switchTab("dashboard");
