import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const fixturePath = process.env.EVIDENCE_AUTHORITY_FIXTURE;
if (!fixturePath) throw new Error("EVIDENCE_AUTHORITY_FIXTURE must point to the real Rust authority QA fixture");
const { createRustAuthorityFixture } = await import(pathToFileURL(fixturePath).href);
const cleanups = [];
let authority, browser, home, root, endpoint, legacyEndpoint;
const children = [];
const origin = "https://evidence.linalab.io";

async function serve(env) {
  const child = Bun.spawn(["bun", "server.mjs"], { cwd: import.meta.dir,
    env: { ...process.env, HOME: home, HOST: "127.0.0.1", PORT: "0", EVIDENCE_ROOT: root,
      UPLOAD_TOKEN: authority.uploadToken, REVIEW_ORIGIN: origin, ...env }, stdout: "pipe", stderr: "pipe" });
  children.push(child);
  const reader = child.stdout.getReader();
  let timer;
  const ready = await Promise.race([reader.read(), new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("Evidence readiness timeout")), 10000);
  })]).finally(() => clearTimeout(timer));
  reader.releaseLock();
  const port = /:(\d+)\//.exec(new TextDecoder().decode(ready.value))?.[1];
  if (!port) throw new Error("Evidence exited before readiness");
  return `http://127.0.0.1:${port}`;
}

beforeAll(async () => {
  authority = await createRustAuthorityFixture(action => cleanups.push(action));
  browser = await authority.login();
  home = await mkdtemp(join(tmpdir(), "evidence-authority-"));
  root = join(home, "data");
  await mkdir(root);
  endpoint = await serve({ EVIDENCE_AUTH_MODE: "central", EVIDENCE_GATEWAY_SECRET_FILE: join(authority.files, "gateway") });
  legacyEndpoint = await serve({ EVIDENCE_AUTH_MODE: "legacy", EVIDENCE_GATEWAY_SECRET_FILE: "" });
}, 30000);

afterAll(async () => {
  for (const child of children) {
    child.kill(); await child.exited;
    const logs = await new Response(child.stderr).text();
    for (const secret of authority.secrets) expect(logs).not.toContain(secret);
  }
  if (home) await rm(home, { recursive: true, force: true });
  for (const action of cleanups.toReversed()) await action();
}, 30000);

function payload(visibility = "private") {
  const form = new FormData();
  form.set("metadata", JSON.stringify({ slug: "authority-proof", title: "Authority proof",
    repository: "https://github.com/example/authority", visibility }));
  form.append("files", new File(["real uploaded bytes"], "assets/proof.txt"));
  return form;
}

async function trusted(lane, method = "POST", path = "/api/evidence") {
  const headers = lane === "browser" ? browser.headers : { authorization: `Bearer ${authority.authority.evidenceMachineKey}` };
  const request = new Request(origin + path, { method, headers });
  const decision = await browser.adapter.authorize(request);
  expect(decision.status).toBe(200);
  const forwarded = await browser.adapter.forwardEvidenceContext(request.headers, decision);
  forwarded.set("authorization", `Bearer ${authority.uploadToken}`);
  return forwarded;
}

async function snapshot() {
  const paths = [];
  async function walk(path, prefix = "") {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      const name = prefix + entry.name;
      if (entry.isDirectory()) { paths.push([name, "directory"]); await walk(join(path, entry.name), name + "/"); }
      else paths.push([name, (await readFile(join(path, entry.name))).toString("base64")]);
    }
  }
  await walk(root);
  return paths.sort(([a], [b]) => a.localeCompare(b));
}

test("browser and machine private multipart uploads persist real files with current upload scope", async () => {
  // Given: real verified principals from Rust and the independent gateway file credential.
  for (const lane of ["browser", "machine"]) {
    // When: actual multipart bytes cross the Evidence HTTP storage boundary.
    const response = await fetch(endpoint + "/api/evidence", { method: "POST", headers: await trusted(lane), body: payload() });
    // Then: stored files and metadata are private, and public access fails.
    expect(response.status).toBe(201);
    const record = await response.json();
    expect(await readFile(join(root, record.slug, "assets/proof.txt"), "utf8")).toBe("real uploaded bytes");
    expect(JSON.parse(await readFile(join(root, record.slug, ".gallery.json"), "utf8")).visibility).toBe("private");
    expect((await fetch(endpoint + `/public/${record.slug}/assets/proof.txt`)).status).toBe(404);
  }
});

test("public multipart metadata is denied before writes when browser publish is withdrawn", async () => {
  // Given: a browser whose POST upload is authorized but publish has just been removed.
  await authority.db.query("UPDATE management.service_grant SET browser_scopes = '[\"evidence:read\",\"evidence:upload\"]' WHERE service_id = 'evidence'");
  const before = await snapshot();
  try {
    // When: public metadata is uploaded despite the allowed POST route.
    const response = await fetch(endpoint + "/api/evidence", { method: "POST", headers: await trusted("browser"), body: payload("public") });
    // Then: denial leaves no staging directory, file or metadata change.
    expect(response.status).toBe(403);
    expect(await snapshot()).toEqual(before);
  } finally {
    await authority.db.query("UPDATE management.service_grant SET browser_scopes = '[\"evidence:read\",\"evidence:upload\",\"evidence:publish\"]' WHERE service_id = 'evidence'");
  }
});

test("browser publish scope permits public multipart publication and visibility changes", async () => {
  // Given: real browser upload and publish authority.
  const response = await fetch(endpoint + "/api/evidence", { method: "POST", headers: await trusted("browser"), body: payload("public") });
  expect(response.status).toBe(201);
  const record = await response.json();
  expect(await (await fetch(endpoint + `/public/${record.slug}/assets/proof.txt`)).text()).toBe("real uploaded bytes");
  const path = `/api/evidence/${record.slug}/visibility`;
  for (const visibility of ["private", "public"]) {
    // When: a browser with current publish authority changes visibility.
    const patch = await fetch(endpoint + path, { method: "PATCH", headers: await trusted("browser", "PATCH", path), body: JSON.stringify({ visibility }) });
    // Then: the persisted visibility and public serving match the decision.
    expect(patch.status).toBe(200);
    expect(JSON.parse(await readFile(join(root, record.slug, ".gallery.json"), "utf8")).visibility).toBe(visibility);
    expect((await fetch(endpoint + `/public/${record.slug}/assets/proof.txt`)).status).toBe(visibility === "public" ? 200 : 404);
  }
});

test("machines never publish public multipart metadata even with a valid upload credential", async () => {
  // Given: genuine machine upload authority, not a mocked scope decision.
  const before = await snapshot();
  // When: a machine submits public metadata through the allowed upload route.
  const response = await fetch(endpoint + "/api/evidence", { method: "POST", headers: await trusted("machine"), body: payload("public") });
  // Then: no bytes or metadata reach storage.
  expect(response.status).toBe(403);
  expect(await snapshot()).toEqual(before);
});

test("forged missing and malformed central context fails closed without filesystem effects", async () => {
  // Given: caller-controlled identity/admin headers and the existing upload transport token.
  const before = await snapshot();
  const valid = await trusted("browser");
  const variants = [new Headers(), new Headers({ "x-linalab-evidence-principal": "browser",
    "x-linalab-evidence-scopes": '["evidence:upload","evidence:publish"]', "x-linalab-evidence-gateway-secret": authority.uploadToken })];
  for (const [name, value] of [["x-linalab-evidence-principal", ""], ["x-linalab-evidence-scopes", "not-json"],
    ["x-linalab-evidence-scopes", '["ci:read"]'], ["x-linalab-evidence-scopes", '["evidence:publish"]'],
    ["x-linalab-evidence-principal", "machine"]]) {
    const headers = new Headers(valid); headers.set(name, value); variants.push(headers);
  }
  for (const headers of variants) {
    headers.set("authorization", `Bearer ${authority.uploadToken}`);
    headers.set("x-linalab-user", "islee@linalab.io"); headers.set("x-linalab-admin", "true");
    // When: unverified context attempts a public multipart upload.
    const response = await fetch(endpoint + "/api/evidence", { method: "POST", headers, body: payload("public") });
    // Then: email, admin and route authorization cannot confer publish rights.
    expect(response.status).toBe(403);
    expect(await snapshot()).toEqual(before);
  }
  const withoutToken = new Headers(valid); withoutToken.delete("authorization");
  expect((await fetch(endpoint + "/api/evidence", { method: "POST", headers: withoutToken, body: payload() })).status).toBe(401);
});

test("PATCH denied without browser publish preserves the exact previous metadata", async () => {
  // Given: a private upload and trusted contexts obtained from real upload decisions.
  const uploaded = await fetch(endpoint + "/api/evidence", { method: "POST", headers: await trusted("browser"), body: payload() });
  const record = await uploaded.json();
  const path = `/api/evidence/${record.slug}/visibility`;
  await authority.db.query("UPDATE management.service_grant SET browser_scopes = '[\"evidence:read\",\"evidence:upload\"]' WHERE service_id = 'evidence'");
  const before = await snapshot();
  try {
    for (const lane of ["browser", "machine"]) {
      // When: an upload-authorized context is used for a visibility PATCH instead.
      const patch = await fetch(endpoint + path, { method: "PATCH", headers: await trusted(lane), body: JSON.stringify({ visibility: "public" }) });
      // Then: Evidence enforces publish again at its own write boundary.
      expect(patch.status).toBe(403);
      expect(await snapshot()).toEqual(before);
    }
  } finally {
    await authority.db.query("UPDATE management.service_grant SET browser_scopes = '[\"evidence:read\",\"evidence:upload\",\"evidence:publish\"]' WHERE service_id = 'evidence'");
  }
});

test("legacy mode keeps token uploads visibility PATCH and REVIEW_ORIGIN compatibility", async () => {
  // Given: explicit legacy mode without an independent gateway context file.
  const headers = { authorization: `Bearer ${authority.uploadToken}` };
  // When: a legacy client submits public multipart metadata and then revokes public access.
  const upload = await fetch(legacyEndpoint + "/api/evidence", { method: "POST", headers, body: payload("public") });
  expect(upload.status).toBe(201);
  const record = await upload.json();
  expect((await fetch(legacyEndpoint + `/api/evidence/${record.slug}/visibility`, { method: "PATCH", headers, body: JSON.stringify({ visibility: "private" }) })).status).toBe(200);
  const hash = "a".repeat(64);
  await writeFile(join(root, record.slug, "manifest.json"), JSON.stringify([{ sha256: hash, name: "proof" }]));
  const review = legacyEndpoint + `/api/reviews?slug=${record.slug}`;
  // Then: the configured public review Origin, not the internal origin, controls review writes.
  expect((await fetch(review, { method: "POST", headers: { origin }, body: JSON.stringify({ sha256: hash, verdict: "pass", note: "reviewed" }) })).status).toBe(200);
  expect((await fetch(review, { method: "POST", headers: { origin: legacyEndpoint }, body: "{}" })).status).toBe(403);
});

test("central opt-in refuses absent short reused and unprotected credential files", async () => {
  // Given: only generated QA secrets and files under this test's temporary directory.
  const paths = [];
  for (const [name, value, mode] of [["short", randomBytes(8).toString("hex"), 0o600],
    ["reused", authority.uploadToken, 0o600], ["unprotected", randomBytes(32).toString("hex"), 0o644]]) {
    const path = join(home, name); await writeFile(path, value, { mode }); paths.push(path);
  }
  for (const path of ["", ...paths]) {
    // When: central mode is enabled without an independent protected >=32-byte credential.
    const child = Bun.spawn(["bun", "-e", 'await import("./storage.mjs")'], { cwd: import.meta.dir,
      env: { ...process.env, EVIDENCE_AUTH_MODE: "central", EVIDENCE_GATEWAY_SECRET_FILE: path, UPLOAD_TOKEN: authority.uploadToken }, stdout: "pipe", stderr: "pipe" });
    // Then: startup fails, and no secret value is logged.
    expect(await child.exited).not.toBe(0);
    const logs = await new Response(child.stderr).text();
    for (const secret of authority.secrets) expect(logs).not.toContain(secret);
  }
});
