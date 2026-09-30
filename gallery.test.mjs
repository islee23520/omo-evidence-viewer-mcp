import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, mkdir, readFile, rm, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

let home;
let server;
let port;

beforeAll(async () => {
  home = await mkdtemp(join(tmpdir(), "gallery-test-"));
  const root = join(home, ".omo/evidence/gallery-public");
  await mkdir(join(root, "alpha-review-20260928"), { recursive: true });
  await mkdir(join(root, "alpha-review-20260927"));
  await mkdir(join(root, "beta-review-20260926"));
  await writeFile(join(root, "alpha-review-20260928/index.html"), "<h1>Alpha review</h1>");
  await writeFile(join(root, "alpha-review-20260927/index.html"), "<h1>Earlier review</h1>");
  await writeFile(join(root, "beta-review-20260926/index.html"), "<h1>Beta review</h1>");
  await writeFile(join(root, "alpha-review-20260928/01-first.png"), "first");
  await writeFile(join(root, "alpha-review-20260928/02-clip.mp4"), "clip");
  await writeFile(join(root, "alpha-review-20260928/03-notes.pdf"), "pdf");
  await writeFile(join(root, "alpha-review-20260928/04-last.webp"), "last");
  server = Bun.spawn(["bun", "server.mjs"], { cwd: import.meta.dir, env: { ...process.env, HOME: home, EVIDENCE_ROOT: root, UPLOAD_TOKEN: "gallery-test-token", HOST: "127.0.0.1", PORT: "0" }, stdout: "pipe", stderr: "pipe" });
  const reader = server.stdout.getReader();
  const { value } = await reader.read();
  port = Number(/:(\d+)\//.exec(new TextDecoder().decode(value))?.[1]);
  reader.releaseLock();
});

afterAll(async () => {
  server?.kill();
  await server?.exited;
  if (home) await rm(home, { recursive: true, force: true });
});

test("authenticated upload publishes nested assets and rejects unsafe paths", async () => {
  function payload(filename = "assets/note.txt") {
    const form = new FormData();
    form.set("metadata", JSON.stringify({ slug: "upload-test", title: "Upload review", repository: "https://github.com/example/upload", labels: ["reviewed"], categories: [] }));
    form.append("files", new File(["uploaded proof"], filename));
    return form;
  }
  const endpoint = `http://127.0.0.1:${port}/api/evidence`;
  expect((await fetch(endpoint, { method: "POST", body: payload() })).status).toBe(401);
  const headers = { Authorization: "Bearer gallery-test-token" };
  const uploaded = await fetch(endpoint, { method: "POST", headers, body: payload() });
  expect(uploaded.status).toBe(201);
  const record = await uploaded.json();
  expect(await (await fetch(`http://127.0.0.1:${port}${record.url}assets/note.txt`)).text()).toBe("uploaded proof");
  for (const filename of ["../secret.txt", "C:/secret.txt", "CON.txt", "assets/../secret.txt"]) {
    expect((await fetch(endpoint, { method: "POST", headers, body: payload(filename) })).status).toBe(400);
  }
  const form = payload();
  form.append("files", new File(["duplicate"], "ASSETS/NOTE.TXT"));
  expect((await fetch(endpoint, { method: "POST", headers, body: form })).status).toBe(400);
});

test("retention deletes expired uploads but preserves fresh and legacy evidence", async () => {
  const root = join(home, ".omo/evidence/gallery-public");
  for (const [name, age] of [["expired-upload", 31], ["fresh-upload", 1]]) {
    await mkdir(join(root, name));
    await writeFile(join(root, name, "proof.txt"), name);
    await writeFile(join(root, name, ".gallery.json"), JSON.stringify({ uploadedAt: new Date(Date.now() - age * 86400000).toISOString() }));
  }
  const cleanup = Bun.spawn(["bun", "storage.mjs"], { cwd: import.meta.dir, env: { ...process.env, EVIDENCE_ROOT: root, RETENTION_DAYS: "30" }, stdout: "pipe", stderr: "pipe" });
  const report = JSON.parse(await new Response(cleanup.stdout).text());
  expect(await cleanup.exited).toBe(0);
  expect(report.removed).toEqual(["expired-upload"]);
  expect((await fetch(`http://127.0.0.1:${port}/evidence/expired-upload/proof.txt`)).status).toBe(404);
  expect((await fetch(`http://127.0.0.1:${port}/evidence/fresh-upload/proof.txt`)).status).toBe(200);
  expect((await fetch(`http://127.0.0.1:${port}/evidence/alpha-review-20260928/index.html`)).status).toBe(200);
});

test("groups legacy evidence by project while links remain live", async () => {
  // Given three public folders across two projects.
  // When a visitor requests the gallery index.
  const page = await (await fetch(`http://127.0.0.1:${port}/`)).text();
  // Then each appears once in a project group.
  expect(page).toContain("<h2>alpha-review</h2>");
  expect(page).toContain("<h2>beta</h2>");
  expect((page.match(/href="\/evidence\/alpha-review-20260928\/"/g) || []).length).toBe(1);
  expect((await fetch(`http://127.0.0.1:${port}/evidence/alpha-review-20260928/index.html`)).status).toBe(200);
});

test("evidence folder includes ordered image, video and PDF pages without replacing file links", async () => {
  const url = `http://127.0.0.1:${port}/evidence/alpha-review-20260928/`;
  const page = await (await fetch(url)).text();
  const items = JSON.parse(/const items=(\[.*?\]);const media=/.exec(page)[1]);
  expect(items).toEqual([
    { name: "01-first.png", href: "01-first.png", kind: "image" },
    { name: "02-clip.mp4", href: "02-clip.mp4", kind: "video" },
    { name: "03-notes.pdf", href: "03-notes.pdf", kind: "pdf" },
    { name: "04-last.webp", href: "04-last.webp", kind: "image" },
  ]);
  expect(page).toContain('<a href="index.html">index.html</a>');
  const video = await fetch(`${url}02-clip.mp4`);
  expect(video.headers.get("content-type")).toBe("video/mp4");
});

test("registration groups repository names, orders creations, displays typed tags, and rejects traversal", async () => {
  // Given a local MCP server pointed at the isolated public root.
  const child = Bun.spawn(["bun", "mcp.mjs"], { cwd: import.meta.dir, env: { ...process.env, HOME: home }, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  const messages = [
    { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } } },
    { jsonrpc: "2.0", method: "notifications/initialized" },
    { jsonrpc: "2.0", id: 2, method: "tools/list" },
    { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "register_evidence", arguments: { slug: "alpha-review-20260928", repository: "git@github.com:Example/Alpha.git", date: "2026-09-25", title: "Homepage audit", labels: ["Reviewed"], categories: ["UI"] } } },
    { jsonrpc: "2.0", id: 4, method: "tools/call", params: { name: "register_evidence", arguments: { slug: "alpha-review-20260927", repository: "https://github.com/example/alpha.git", date: "2026-09-29", title: "Earlier audit", labels: [], categories: [] } } },
    { jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "register_evidence", arguments: { slug: "../private", repository: "https://github.com/example/alpha", title: "Bad", labels: [], categories: [] } } },
  ];
  // When a client completes the handshake and calls the tool.
  child.stdin.write(messages.map(message => JSON.stringify(message)).join("\n") + "\n");
  child.stdin.end();
  const output = await new Response(child.stdout).text();
  await child.exited;
  const replies = output.trim().split("\n").map(JSON.parse);
  // Then the valid label appears on the live index; unsafe input is rejected.
  expect(replies[1].result.tools[0].name).toBe("register_evidence");
  expect(replies[1].result.tools[0].inputSchema.required).toContain("repository");
  expect(replies[2].result.structuredContent.repository).toBe("https://github.com/example/alpha");
  expect(replies[2].result.structuredContent.tags).toEqual([{ type: "label", value: "Reviewed" }, { type: "category", value: "UI" }]);
  expect(replies[4].result.isError).toBe(true);
  const root = join(home, ".omo/evidence/gallery-public");
  const old = new Date("2026-09-20T00:00:00Z");
  const recent = new Date("2026-09-22T00:00:00Z");
  const firstPath = join(root, "alpha-review-20260928/.gallery.json");
  const secondPath = join(root, "alpha-review-20260927/.gallery.json");
  const metadata = JSON.parse(await readFile(firstPath, "utf8"));
  const second = JSON.parse(await readFile(secondPath, "utf8"));
  metadata.createdAt = "2026-09-24T00:00:00.000Z";
  second.createdAt = "2026-09-21T00:00:00.000Z";
  metadata.updatedAt = "2026-09-25T00:00:00.000Z";
  second.updatedAt = "2026-09-29T00:00:00.000Z";
  await writeFile(firstPath, JSON.stringify(metadata));
  await writeFile(secondPath, JSON.stringify(second));
  await utimes(join(root, "alpha-review-20260928"), old, old);
  await utimes(join(root, "alpha-review-20260927"), recent, recent);
  await utimes(join(root, "beta-review-20260926"), old, old);
  await writeFile(join(root, "beta-review-20260926/.gallery.json"), JSON.stringify({ project: "beta", createdAt: "2026-09-20T00:00:00.000Z" }));
  expect(metadata.tags).toEqual(replies[2].result.structuredContent.tags);
  const page = await (await fetch(`http://127.0.0.1:${port}/`)).text();
  expect((page.match(/<h2>alpha<\/h2>/g) || []).length).toBe(1);
  expect(page).toContain("Homepage audit");
  expect(page).toContain("라벨 · Reviewed");
  expect(page).toContain("카테고리 · UI");
  expect(page.indexOf("Homepage audit")).toBeLessThan(page.indexOf("Earlier audit"));
  expect(page.indexOf("<h2>alpha</h2>")).toBeLessThan(page.indexOf("<h2>beta</h2>"));
});
