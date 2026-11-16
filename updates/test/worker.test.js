// The update service against a stand-in for GitHub: `node --test` in this folder.
import assert from "node:assert/strict";
import { timingSafeEqual } from "node:crypto";
import { afterEach, beforeEach, test } from "node:test";
import worker from "../src/index.js";

// The Workers runtime has this; Node does not.
// @ts-ignore
crypto.subtle.timingSafeEqual ??= (a, b) => timingSafeEqual(Buffer.from(a), Buffer.from(b));

const env = { GITHUB_REPO: "someone/show", GITHUB_TOKEN: "the-token", UPDATE_KEY: "the-key" };
const API = "https://api.github.com/repos/someone/show";
const manifest = JSON.stringify({ version: "0.1.2", platform: "macos-arm64" });

/** What the stand-in holds: one release, and the files in it. */
const release = {
  tag_name: "v0.1.2",
  assets: [
    { name: "update-macos-arm64.json", url: `${API}/releases/assets/1`, size: manifest.length },
    { name: "update-macos-arm64.json.sig", url: `${API}/releases/assets/2`, size: 129 },
    { name: "PTZ-Programmer-0.1.2-macos-arm64.pkg", url: `${API}/releases/assets/3`, size: 13_000_000 },
  ],
};
const files = { 1: manifest, 2: "ab".repeat(64) + "\n" };

let asked = [];
let token = "the-token";
const real = globalThis.fetch;

beforeEach(() => {
  asked = [];
  token = "the-token";
  globalThis.fetch = async (input, init = {}) => {
    const url = String(input);
    const sent = new Headers(init.headers);
    asked.push({ url, authorization: sent.get("authorization") });
    if (url.startsWith("https://files.example/")) {
      // The signed address takes no token, and must not be sent one.
      assert.equal(sent.get("authorization"), null);
      return new Response(files[url.split("/").pop()]);
    }
    if (sent.get("authorization") !== `Bearer ${token}`) {
      return new Response("{}", { status: 401 });
    }
    if (url === `${API}/releases/latest` || url === `${API}/releases/tags/v0.1.2`) {
      return Response.json(release);
    }
    const asset = url.match(/\/releases\/assets\/(\d)$/);
    if (asset) {
      assert.equal(sent.get("accept"), "application/octet-stream");
      assert.equal(init.redirect, "manual");
      return new Response(null, { status: 302, headers: { location: `https://files.example/${asset[1]}` } });
    }
    return new Response("{}", { status: 404 });
  };
});

afterEach(() => {
  globalThis.fetch = real;
});

const get = (path, key = "the-key", method = "GET") =>
  worker.fetch(new Request(`https://updates.example${path}`, { method, headers: key ? { "x-ptz-key": key } : {} }), env);

test("the newest release is described to an app that sends the key, word for word", async () => {
  const response = await get("/v1/latest/macos-arm64");
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { manifest, signature: "ab".repeat(64) });
});

test("nobody without the key is told anything, and nothing is asked of GitHub for them", async () => {
  for (const key of ["", "the-keY", "the-key-and-more"]) {
    const response = await get("/v1/latest/macos-arm64", key);
    assert.equal(response.status, 401);
  }
  assert.equal((await get("/v1/download/0.1.2/PTZ-Programmer-0.1.2-macos-arm64.pkg", "")).status, 401);
  assert.deepEqual(asked, []);
  assert.equal((await get("/", "")).status, 200);
  assert.equal((await get("/v1/latest/macos-arm64", "the-key", "POST")).status, 405);
});

test("an installer is handed over as the address GitHub signs for it", async () => {
  const response = await get("/v1/download/0.1.2/PTZ-Programmer-0.1.2-macos-arm64.pkg");
  assert.equal(response.status, 302);
  assert.equal(response.headers.get("location"), "https://files.example/3");
});

test("only the installers of a release can be asked for", async () => {
  for (const path of [
    "/v1/download/0.1.2/update-macos-arm64.json",
    "/v1/download/0.1.2/PTZ-Programmer-0.1.1-macos-arm64.pkg",
    "/v1/download/0.1.2/PTZ-Server-0.1.2-macos-arm64.pkg",
    "/v1/download/0.1.9/PTZ-Programmer-0.1.9-macos-arm64.pkg",
    "/v1/download/0.1.2/..%2F..%2Fsecrets",
    "/v1/latest/windows-x86_64",
    "/v1/elsewhere",
  ]) {
    assert.equal((await get(path)).status, 404, path);
  }
});

test("a token GitHub no longer takes is said in words the app can show", async () => {
  token = "another";
  const response = await get("/v1/latest/macos-arm64");
  assert.equal(response.status, 502);
  assert.match((await response.json()).error, /refused the update service's token/);
});

test("a service with no secrets says it is not set up", async () => {
  const response = await worker.fetch(new Request("https://updates.example/v1/latest/macos-arm64"), { GITHUB_REPO: "someone/show" });
  assert.equal(response.status, 503);
});
