const HOSTS = ["api.traintime.ch", "api1.traintime.ch", "api2.traintime.ch"];
const STATION = "8503000";
const USER_AGENT = "traintime-monitor/1";
const REQUEST_TIMEOUT_MS = 8000;
const RETRY_DELAY_MS = 5000;
const STALE_MS = 30 * 60 * 1000;
const DOWN_AFTER_FAILS = 2;
const REMINDER_HOUR = 8;
const TIME_ZONE = "Europe/Zurich";

export default {
  async scheduled(_event, env, ctx) {
    ctx.waitUntil(run(env));
  },
};

export async function run(env, now = Date.now(), deps = {}) {
  const fetchFn = deps.fetch ?? fetch;
  const sleep = deps.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  // api and api2 are one Worker. Probes that land together are the burst that
  // broke it on 2026-09-18, so hosts are checked one after another.
  const results = [];
  for (const host of HOSTS) {
    results.push(await probeHost(host, env.API_KEY, now, fetchFn, sleep));
  }
  const checks = HOSTS.map((host, i) => ({
    key: host,
    ok: results[i].failures.length === 0,
    detail: results[i].failures.join("; "),
    severity: "down",
  }));
  const parity = compareParity(results[0].departures, results[1].departures);
  checks.push({
    key: "parity",
    ok: parity === null,
    detail: parity ?? "",
    severity: "warning",
  });
  for (const check of checks) {
    const prev = await readState(env.STATE, check.key);
    const { state, push } = transition(prev, check.ok, now);
    // A push that did not land keeps the old state, so the next run sends it again.
    if (push && !(await notify(env, message(check, push, prev, now), fetchFn))) {
      continue;
    }
    // The free plan allows 1,000 KV writes a day and a run every five minutes has four checks.
    if (JSON.stringify(state) !== JSON.stringify(prev)) {
      await env.STATE.put(`status:${check.key}`, JSON.stringify(state));
    }
  }
}

// --- Probes ---

// One host's result: an empty failures list means every probe passed.
export async function probeHost(host, apiKey, now, fetchFn, sleep) {
  const base = `https://${host}`;
  const get = (path, auth) => fetchFn(base + path, requestInit(apiKey, auth));
  const failures = [];
  let departures = [];
  const attempt = async (name, probe) => {
    let error = await probe().catch((e) => `${name}: ${e.message}`);
    if (error === null) return true;
    await sleep(RETRY_DELAY_MS);
    error = await probe().catch((e) => `${name}: ${e.message}`);
    if (error === null) return true;
    failures.push(error);
    return false;
  };
  await attempt("health", () => checkHealth(get));
  await attempt("departures", async () => {
    const r = await checkDepartures(get, now);
    if (typeof r === "string") return r;
    departures = r;
    return null;
  });
  await attempt("nearby", () => checkNearby(get));
  const train = departures.find((d) => d.trainNumber);
  if (train) {
    await attempt("formation", () => checkFormation(get, train, now));
  }
  return { host, failures, departures };
}

function requestInit(apiKey, auth) {
  const headers = { "User-Agent": USER_AGENT };
  if (auth) headers["X-API-Key"] = apiKey;
  return { headers, signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) };
}

async function checkHealth(get) {
  const resp = await get("/health", false);
  if (resp.status !== 200) return `health: HTTP ${resp.status}`;
  const body = await json(resp);
  if (body?.status !== "ok") return "health: body is not {\"status\":\"ok\"}";
  return null;
}

async function checkDepartures(get, now) {
  const resp = await get(`/v1/departures?id=${STATION}&limit=5`, true);
  if (resp.status !== 200) return `departures: HTTP ${resp.status}`;
  const body = await json(resp);
  const deps = body?.departures;
  if (!Array.isArray(deps) || deps.length === 0) return "departures: empty";
  const offset = deps[0].departure * 1000 - now;
  if (Math.abs(offset) > STALE_MS) {
    return `departures: first departure ${Math.round(offset / 60000)} min from now`;
  }
  return deps;
}

async function checkNearby(get) {
  const resp = await get(`/v1/nearby?lat=47.3779&lon=8.5403&mode=train`, true);
  if (resp.status !== 200) return `nearby: HTTP ${resp.status}`;
  const body = await json(resp);
  const station = body?.train?.find?.((s) => s.id === STATION);
  if (!station) return `nearby: station ${STATION} missing`;
  if (!Array.isArray(station.departures)) return "nearby: no departures array";
  return null;
}

async function checkFormation(get, train, now) {
  const params = new URLSearchParams({
    train: train.trainNumber,
    date: localDate(now),
    stop: STATION,
    operatorRef: train.operatorRef ?? "",
  });
  const resp = await get(`/v1/formation?${params}`, true);
  if (resp.status !== 200 && resp.status !== 404) {
    return `formation: HTTP ${resp.status}`;
  }
  const body = await json(resp);
  if (body === undefined) return "formation: body is not JSON";
  return null;
}

// The caches behind api and api1 are independent, so delay may differ and one
// list may start a train earlier than the other. Inside the window both lists
// cover, the timetable itself must match.
export function compareParity(a, b) {
  if (a.length === 0 || b.length === 0) return null;
  const rowsA = a.slice(0, 5);
  const rowsB = b.slice(0, 5);
  const from = Math.max(rowsA[0].departure, rowsB[0].departure);
  const to = Math.min(rowsA.at(-1).departure, rowsB.at(-1).departure);
  const key = (d) => `${d.number} ${d.trainNumber} @${d.departure}`;
  const inWindow = (d) => d.departure >= from && d.departure <= to;
  const keysA = new Set(rowsA.filter(inWindow).map(key));
  const keysB = new Set(rowsB.filter(inWindow).map(key));
  for (const k of keysA) if (!keysB.has(k)) return `api has ${k}, api1 does not`;
  for (const k of keysB) if (!keysA.has(k)) return `api1 has ${k}, api does not`;
  return null;
}

async function json(resp) {
  try {
    return await resp.json();
  } catch {
    return undefined;
  }
}

// --- State ---

// Down needs two failed runs in a row, recovery the first green one, and a
// host that stays down gets a single reminder at 08:00.
export function transition(prev, ok, now) {
  const state = { ...prev };
  if (ok) {
    state.fails = 0;
    if (prev.status === "down") {
      state.status = "ok";
      state.since = now;
      return { state, push: "recovered" };
    }
    return { state, push: null };
  }
  if (prev.status === "down") {
    const today = localDate(now);
    if (localHour(now) === REMINDER_HOUR && state.reminded !== today) {
      state.reminded = today;
      return { state, push: "reminder" };
    }
    return { state, push: null };
  }
  state.fails = (prev.fails ?? 0) + 1;
  if (state.fails >= DOWN_AFTER_FAILS) {
    state.status = "down";
    state.since = now;
    return { state, push: "down" };
  }
  return { state, push: null };
}

async function readState(kv, key) {
  const raw = await kv.get(`status:${key}`);
  return raw ? JSON.parse(raw) : { status: "ok", since: 0, fails: 0 };
}

// --- Notifications ---

export function message(check, push, prev, now) {
  const name = check.key === "parity" ? "api vs api1 parity" : check.key;
  const priority = push === "recovered" || check.severity === "warning" ? "default" : "urgent";
  if (push === "recovered") {
    const mins = Math.round((now - prev.since) / 60000);
    return {
      title: `${name} ${check.severity === "warning" ? "agrees again" : "recovered"}`,
      body: `after ${mins} min`,
      priority,
      tags: "white_check_mark",
    };
  }
  const verb = check.severity === "warning" ? "mismatch" : "down";
  const title = push === "reminder" ? `${name} still ${verb}` : `${name} ${verb}`;
  return {
    title,
    body: check.detail,
    priority,
    tags: check.severity === "warning" ? "warning" : "rotating_light",
  };
}

async function notify(env, msg, fetchFn) {
  const headers = {
    "User-Agent": USER_AGENT,
    Title: msg.title,
    Priority: msg.priority,
    Tags: msg.tags,
  };
  if (env.NTFY_TOKEN) headers.Authorization = `Bearer ${env.NTFY_TOKEN}`;
  try {
    const resp = await fetchFn(`https://ntfy.sh/${env.NTFY_TOPIC.trim()}`, {
      method: "POST",
      headers,
      body: msg.body,
    });
    if (resp.status >= 200 && resp.status < 300) return true;
    const detail = ((await resp.text?.().catch(() => "")) ?? "").slice(0, 120);
    console.log(`ntfy push failed: ${resp.status} ${detail}`);
  } catch (err) {
    console.log(`ntfy push failed: ${err.message}`);
  }
  return false;
}

// --- Time ---

export function localDate(ms) {
  return new Intl.DateTimeFormat("en-CA", {
    timeZone: TIME_ZONE,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).format(new Date(ms));
}

export function localHour(ms) {
  const hour = new Intl.DateTimeFormat("en-GB", {
    timeZone: TIME_ZONE,
    hour: "2-digit",
    hourCycle: "h23",
  }).format(new Date(ms));
  return Number(hour);
}
