import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

let root, gallery, mock, base;
const calls = [];
let fail = false;
beforeAll(async () => {
  root = await mkdtemp(join(tmpdir(), "ggui-gallery-"));
  mock = Bun.serve({ port: 0, hostname: "127.0.0.1", async fetch(request) {
    expect(request.headers.get("authorization")).toBe("Bearer ggui-test");
    const message = await request.json();
    if (message.method === "initialize") return Response.json({ jsonrpc: "2.0", id: message.id, result: { protocolVersion: "2025-06-18" } }, { headers: { "Mcp-Session-Id": "test-session" } });
    expect(request.headers.get("mcp-session-id")).toBe("test-session");
    if (message.method === "notifications/initialized") return new Response(null, { status: 202 });
    calls.push(message.params);
    let structuredContent;
    if (message.params.name === "ggui_handshake") structuredContent = { handshakeId: "handshake", action: "create" };
    if (message.params.name === "ggui_render") structuredContent = { outcome: fail ? "failed" : "rendered", sessionId: "session" };
    if (message.params.name === "ggui_get_render_source") structuredContent = { blueprint: { source: 'import React from "react";export default function Viewer({title,documents}){return <main><h1>{title}</h1>{documents.map(d=><section key={d.name}><a href={d.href}>{d.name}</a><div dangerouslySetInnerHTML={{__html:d.html}}/>{d.sheets.map(s=><table key={s.name}><tbody>{s.rows.map((r,i)=><tr key={i}>{r.map((c,j)=><td key={j}>{c}</td>)}</tr>)}</tbody></table>)}</section>)}</main>}' } };
    const source = 'import React from "react";export default function Viewer({title}){return React.createElement("h1",null,title)}';
    const data = JSON.stringify({ jsonrpc: "2.0", id: message.id, result: { structuredContent, ...(message.params.name === "ggui_render" ? { _meta: { "ai.ggui/render": { codeB64: Buffer.from(source).toString("base64") } } } : {}) } });
    // Keep the stream open: the client must stop at its exact RPC response.
    return new Response(new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode(`event: message\ndata: ${data}\n\n`)); } }), { headers: { "Content-Type": "text/event-stream" } });
  } });
  gallery = Bun.spawn(["bun", "server.mjs"], { cwd: import.meta.dir, env: { ...process.env, HOME: root, EVIDENCE_ROOT: root, HOST: "127.0.0.1", PORT: "0", UPLOAD_TOKEN: "test", GGUI_MCP_URL: `${mock.url}mcp`, GGUI_MCP_TOKEN: "ggui-test" }, stdout: "pipe", stderr: "pipe" });
  const reader = gallery.stdout.getReader();
  const { value } = await reader.read();
  base = /http:\/\/[^\s]+/.exec(new TextDecoder().decode(value))[0];
  reader.releaseLock();
});
afterAll(async () => { gallery?.kill(); await gallery?.exited; mock?.stop(true); if (root) await rm(root, { recursive: true, force: true }); });
async function upload(files) {
  const body = new FormData();
  body.set("metadata", JSON.stringify({ slug: "documents", title: "Document review", repository: "https://github.com/example/documents", visibility: "public" }));
  for (const file of files) body.append("files", file);
  return fetch(`${base}api/evidence`, { method: "POST", headers: { Authorization: "Bearer test" }, body });
}
test("document upload calls GGUI and publishes a self-contained viewer with original files", async () => {
  const response = await upload([new File(["# Document proof"], "notes.md"), new File(['name,value\n"comma,value",42'], "data.csv"), new File(["%PDF-1.4"], "proof.pdf")]);
  expect(response.status).toBe(201);
  const record = await response.json();
  expect(record.documentViewer.files).toEqual(["notes.md", "data.csv", "proof.pdf"]);
  const render = calls.find(call => call.name === "ggui_render");
  expect(render.arguments.props.documents[1].sheets[0].rows[1]).toEqual(["comma,value", "42"]);
  const html = await (await fetch(new URL(record.url, base))).text();
  expect(html).toContain('<div id="viewer">');
  expect(html).toContain('type="module"');
  expect(await (await fetch(new URL(record.url + "notes.md", base))).text()).toBe("# Document proof");
});
test("existing index HTML is preserved and does not call GGUI", async () => {
  const count = calls.length;
  const response = await upload([new File(["<h1>Authored page</h1>"], "index.html"), new File(["# Notes"], "notes.md")]);
  const record = await response.json();
  expect(response.status).toBe(201);
  expect(calls.length).toBe(count);
  expect(await (await fetch(new URL(record.url + "index.html?raw=1", base))).text()).toBe("<h1>Authored page</h1>");
});
test("GGUI failure leaves no published folder or staging data", async () => {
  const before = (await readdir(root)).sort();
  fail = true;
  try { expect((await upload([new File(["# Failed"], "notes.md")])).status).toBe(502); }
  finally { fail = false; }
  expect((await readdir(root)).sort()).toEqual(before);
});
