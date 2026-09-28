import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
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
  server = Bun.spawn(["bun", "server.mjs"], { cwd: import.meta.dir, env: { ...process.env, HOME: home, HOST: "127.0.0.1", PORT: "0" }, stdout: "pipe", stderr: "pipe" });
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

test("groups legacy evidence by project and newest date while links remain live", async () => {
  // Given three public folders across two projects.
  // When a visitor requests the gallery index.
  const page = await (await fetch(`http://127.0.0.1:${port}/`)).text();
  // Then each appears once in a project group, newest date first.
  expect(page.indexOf("<h2>alpha-review</h2>")).toBeLessThan(page.indexOf("<h2>beta</h2>"));
  expect(page.indexOf("2026-09-28")).toBeLessThan(page.indexOf("2026-09-27"));
  expect((page.match(/href="\/evidence\/alpha-review-20260928\/"/g) || []).length).toBe(1);
  expect((await fetch(`http://127.0.0.1:${port}/evidence/alpha-review-20260928/index.html`)).status).toBe(200);
});

test("registration labels an existing public folder and rejects traversal", async () => {
  // Given a local MCP server pointed at the isolated public root.
  const child = Bun.spawn(["bun", "mcp.mjs"], { cwd: import.meta.dir, env: { ...process.env, HOME: home }, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  const messages = [
    { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } } },
    { jsonrpc: "2.0", method: "notifications/initialized" },
    { jsonrpc: "2.0", id: 2, method: "tools/list" },
    { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "register_evidence", arguments: { slug: "alpha-review-20260928", project: "Alpha Studio", date: "2026-09-25", title: "Homepage audit" } } },
    { jsonrpc: "2.0", id: 4, method: "tools/call", params: { name: "register_evidence", arguments: { slug: "../private", project: "Bad", date: "2026-09-28", title: "Bad" } } },
  ];
  // When a client completes the handshake and calls the tool.
  child.stdin.write(messages.map(message => JSON.stringify(message)).join("\n") + "\n");
  child.stdin.end();
  const output = await new Response(child.stdout).text();
  await child.exited;
  const replies = output.trim().split("\n").map(JSON.parse);
  // Then the valid label appears on the live index; unsafe input is rejected.
  expect(replies[1].result.tools[0].name).toBe("register_evidence");
  expect(replies[2].result.structuredContent.project).toBe("Alpha Studio");
  expect(replies[3].result.isError).toBe(true);
  const page = await (await fetch(`http://127.0.0.1:${port}/`)).text();
  expect(page).toContain("<h2>Alpha Studio</h2>");
  expect(page).toContain("Homepage audit");
});
