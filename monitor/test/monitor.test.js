import { test } from "node:test";
import assert from "node:assert/strict";

import { compareParity, localDate, localHour, message, probeHost, run, transition } from "../src/index.js";

const NOW = Date.UTC(2026, 8, 17, 10, 0, 0);
const OK = { status: "ok", since: 0, fails: 0 };
const DOWN = { status: "down", since: NOW - 12 * 60000, fails: 2 };

function departure(i) {
  return { number: "S8", trainNumber: String(18870 + i), departure: (NOW + i * 60000) / 1000, operatorRef: "ojp:11" };
}

function responses(overrides = {}) {
  const deps = [0, 1, 2, 3, 4].map(departure);
  return {
    "/health": [200, { status: "ok" }],
    "/v1/departures": [200, { departures: deps }],
    "/v1/nearby": [200, { train: [{ id: "8503000", departures: deps }] }],
    "/v1/formation": [404, { error: "not found" }],
    ...overrides,
  };
}

function fakeFetch(table, calls = []) {
  return async (url, init) => {
    calls.push({ url, init });
    const path = new URL(url).pathname;
    const entry = table[path];
    if (entry === undefined) throw new Error("connect timeout");
    const [status, body] = typeof entry === "function" ? entry() : entry;
    return {
      status,
      json: async () => {
        if (typeof body === "string") throw new SyntaxError("bad json");
        return body;
      },
    };
  };
}

const noSleep = async () => {};

test("probe passes on a healthy host", async () => {
  const calls = [];
  const r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch(responses(), calls), noSleep);
  assert.deepEqual(r.failures, []);
  assert.equal(r.departures.length, 5);
  assert.equal(calls.length, 4);
  assert.equal(calls[0].init.headers["X-API-Key"], undefined);
  assert.equal(calls[1].init.headers["X-API-Key"], "k");
  assert.equal(calls[1].init.headers["User-Agent"], "traintime-monitor/1");
  assert.match(calls[3].url, /train=18870&date=2026-09-17&stop=8503000&operatorRef=ojp%3A11/);
});

test("probe retries once then records the failure", async () => {
  const calls = [];
  const slept = [];
  const table = responses({ "/health": [503, {}] });
  const r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch(table, calls), async (ms) => slept.push(ms));
  assert.deepEqual(r.failures, ["health: HTTP 503"]);
  assert.deepEqual(slept, [5000]);
  assert.equal(calls.filter((c) => c.url.endsWith("/health")).length, 2);
});

test("probe recovers when the retry succeeds", async () => {
  let first = true;
  const table = responses({
    "/health": () => {
      const s = first ? 500 : 200;
      first = false;
      return [s, { status: "ok" }];
    },
  });
  const r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch(table), noSleep);
  assert.deepEqual(r.failures, []);
});

test("stale departures fail", async () => {
  const stale = [departure(0)];
  stale[0].departure = (NOW - 31 * 60000) / 1000;
  const table = responses({ "/v1/departures": [200, { departures: stale }] });
  const r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch(table), noSleep);
  assert.deepEqual(r.failures, ["departures: first departure -31 min from now"]);
  assert.deepEqual(r.departures, []);
});

test("empty departures and missing station fail", async () => {
  const table = responses({
    "/v1/departures": [200, { departures: [] }],
    "/v1/nearby": [200, { train: [{ id: "8503016", departures: [] }] }],
  });
  const r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch(table), noSleep);
  assert.deepEqual(r.failures, ["departures: empty", "nearby: station 8503000 missing"]);
});

test("formation accepts 404 but not 5xx or a non-JSON body", async () => {
  let r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch(responses({ "/v1/formation": [502, "<html>"] })), noSleep);
  assert.deepEqual(r.failures, ["formation: HTTP 502"]);
  r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch(responses({ "/v1/formation": [200, "<html>"] })), noSleep);
  assert.deepEqual(r.failures, ["formation: body is not JSON"]);
});

test("network error is a failure, not a crash", async () => {
  const r = await probeHost("api.traintime.ch", "k", NOW, fakeFetch({}), noSleep);
  assert.deepEqual(r.failures, [
    "health: connect timeout",
    "departures: connect timeout",
    "nearby: connect timeout",
  ]);
});

test("parity ignores delay, empty lists and a shifted window", () => {
  const a = [0, 1, 2, 3, 4, 5].map(departure);
  const b = a.map((d) => ({ ...d, delay: 3 }));
  assert.equal(compareParity(a, b), null);
  assert.equal(compareParity([], b), null);
  assert.equal(compareParity(a, a.slice(1)), null);
  b[5].trainNumber = "x";
  assert.equal(compareParity(a, b), null);
  b[2].trainNumber = "x";
  assert.equal(compareParity(a, b), "api has S8 18872 @1789639320, api1 does not");
  const c = a.map((d) => ({ ...d, departure: d.departure + 60 }));
  assert.match(compareParity(a, c), /^api has S8 18871 @/);
});

test("down needs two failed runs", () => {
  let { state, push } = transition(OK, false, NOW);
  assert.equal(push, null);
  assert.equal(state.status, "ok");
  assert.equal(state.fails, 1);
  ({ state, push } = transition(state, false, NOW));
  assert.equal(push, "down");
  assert.equal(state.status, "down");
  assert.equal(state.since, NOW);
});

test("one green run resets the fail count", () => {
  const once = transition(OK, false, NOW).state;
  const { state, push } = transition(once, true, NOW);
  assert.equal(push, null);
  assert.equal(state.fails, 0);
});

test("recovery pushes on the first green run", () => {
  const { state, push } = transition(DOWN, true, NOW);
  assert.equal(push, "recovered");
  assert.equal(state.status, "ok");
});

test("a host that stays down is silent except at 08:00 once a day", () => {
  const eight = Date.UTC(2026, 8, 17, 6, 0, 0);
  assert.equal(localHour(eight), 8);
  let { state, push } = transition(DOWN, false, NOW);
  assert.equal(push, null);
  ({ state, push } = transition(DOWN, false, eight));
  assert.equal(push, "reminder");
  assert.equal(state.reminded, "2026-09-17");
  ({ state, push } = transition(state, false, eight + 5 * 60000));
  assert.equal(push, null);
  ({ state, push } = transition(state, false, eight + 24 * 3600000));
  assert.equal(push, "reminder");
});

test("messages carry priority and duration", () => {
  const host = { key: "api1.traintime.ch", severity: "down", detail: "health: HTTP 503" };
  assert.deepEqual(message(host, "down", OK, NOW), {
    title: "api1.traintime.ch down",
    body: "health: HTTP 503",
    priority: "urgent",
    tags: "rotating_light",
  });
  assert.deepEqual(message(host, "recovered", DOWN, NOW), {
    title: "api1.traintime.ch recovered",
    body: "after 12 min",
    priority: "default",
    tags: "white_check_mark",
  });
  assert.equal(message(host, "reminder", DOWN, NOW).title, "api1.traintime.ch still down");
  const parity = { key: "parity", severity: "warning", detail: "row 1: ..." };
  const warn = message(parity, "down", OK, NOW);
  assert.equal(warn.title, "api vs api1 parity mismatch");
  assert.equal(warn.priority, "default");
});

test("run pushes once per transition and stores state", async () => {
  const store = new Map();
  const env = {
    API_KEY: "k",
    NTFY_TOPIC: "t0pic",
    NTFY_TOKEN: "tk",
    STATE: { get: async (k) => store.get(k) ?? null, put: async (k, v) => store.set(k, v) },
  };
  const table = responses();
  const calls = [];
  const fetchFn = async (url, init) => {
    if (url.startsWith("https://ntfy.sh/")) {
      calls.push({ url, init });
      return { status: 200, json: async () => ({}) };
    }
    if (new URL(url).hostname === "api1.traintime.ch") throw new Error("tunnel down");
    return fakeFetch(table)(url, init);
  };
  const deps = { fetch: fetchFn, sleep: noSleep };
  await run(env, NOW, deps);
  assert.equal(calls.length, 0);
  await run(env, NOW + 300000, deps);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, "https://ntfy.sh/t0pic");
  assert.equal(calls[0].init.headers.Title, "api1.traintime.ch down");
  assert.equal(calls[0].init.headers.Authorization, "Bearer tk");
  assert.equal(calls[0].init.headers["User-Agent"], "traintime-monitor/1");
  assert.equal(JSON.parse(store.get("status:api1.traintime.ch")).status, "down");
  assert.equal(store.has("status:parity"), false);
  await run(env, NOW + 600000, deps);
  assert.equal(calls.length, 1);
});

test("a push ntfy rejects is sent again on the next run", async () => {
  const store = new Map();
  const env = {
    API_KEY: "k",
    NTFY_TOPIC: "t0pic",
    STATE: { get: async (k) => store.get(k) ?? null, put: async (k, v) => store.set(k, v) },
  };
  const table = responses();
  let ntfyStatus = 429;
  let pushes = 0;
  const fetchFn = async (url, init) => {
    if (url.startsWith("https://ntfy.sh/")) {
      pushes += 1;
      return { status: ntfyStatus, json: async () => ({}) };
    }
    if (new URL(url).hostname === "api1.traintime.ch") throw new Error("tunnel down");
    return fakeFetch(table)(url, init);
  };
  const deps = { fetch: fetchFn, sleep: noSleep };
  await run(env, NOW, deps);
  await run(env, NOW + 300000, deps);
  assert.equal(pushes, 1);
  assert.equal(JSON.parse(store.get("status:api1.traintime.ch")).status, "ok");
  ntfyStatus = 200;
  await run(env, NOW + 600000, deps);
  assert.equal(pushes, 2);
  assert.equal(JSON.parse(store.get("status:api1.traintime.ch")).status, "down");
});

test("run writes nothing while every host stays ok", async () => {
  const writes = [];
  const env = {
    API_KEY: "k",
    NTFY_TOPIC: "t0pic",
    STATE: { get: async () => null, put: async (k) => writes.push(k) },
  };
  const deps = { fetch: fakeFetch(responses()), sleep: noSleep };
  await run(env, NOW, deps);
  await run(env, NOW + 300000, deps);
  assert.deepEqual(writes, []);
});

test("local date follows Zurich", () => {
  assert.equal(localDate(Date.UTC(2026, 8, 17, 22, 30)), "2026-09-18");
});
