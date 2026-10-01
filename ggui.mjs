import { resolve } from "node:path";
import { getCssTokens } from "@ggui-ai/design/rendering";
import { prepareDocuments } from "./document-data.mjs";

const documentTypes = /\.(md|markdown|pdf|xlsx|csv)$/i;
const intent = "Read-only responsive Korean document viewer: document tabs, sanitized Markdown html with readable typography, PDF iframe using relative href and original download, CSV/XLSX sheet tabs and searchable scrollable tables with sticky column headers. Render every supplied value without summarizing or dropping rows. Local React state only, no actions, no remote data, no fetching or external links. Props: title, documents with name/href/kind/html/sheets{name,rows}. html is sanitized by the server. Use the GGUI design system. Desktop and mobile must fit their viewport; tables own horizontal scrolling.";
const rowSchema = { type: "array", items: { type: "array", items: { type: "string" } } };
const contract = { propsSpec: { properties: {
  title: { schema: { type: "string" }, required: true },
  documents: { required: true, schema: { type: "array", items: { type: "object", properties: {
    name: { type: "string" }, href: { type: "string" }, kind: { type: "string" }, html: { type: "string" },
    sheets: { type: "array", items: { type: "object", properties: { name: { type: "string" }, rows: rowSchema } } },
  } } } },
} } };

export async function bundleViewer(source, props, loader = "tsx") {
  const component = "evidence-generated-viewer";
  const entry = resolve(import.meta.dir, "ggui-client.jsx");
  const json = JSON.stringify(props).replaceAll("<", "\\u003c");
  const result = await Bun.build({
    entrypoints: [entry], target: "browser", format: "esm", minify: true,
    define: { "process.env.NODE_ENV": '"production"' },
    plugins: [{ name: "document-viewer-source", setup(build) {
      build.onResolve({ filter: /^evidence-generated-viewer$/ }, () => ({ path: component, namespace: "viewer" }));
      build.onLoad({ filter: /.*/, namespace: "viewer" }, () => ({ contents: source, loader, resolveDir: import.meta.dir }));
      build.onResolve({ filter: /.*/ }, args => {
        if (args.importer === component && !["react", "react/jsx-runtime", "react/jsx-dev-runtime", "@ggui-ai/design"].includes(args.path)) {
          throw new Error(`Unsupported viewer import: ${args.path}`);
        }
      });
    } }],
  });
  if (!result.success) throw new AggregateError(result.logs, "GGUI viewer compilation failed");
  const script = (await result.outputs[0].text()).replaceAll("</script", "<\\/script");
  const title = props.title.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll('"', "&quot;");
  return `<!doctype html><html lang="ko"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${title}</title><style>${getCssTokens()}body{margin:0;font-family:system-ui}*{box-sizing:border-box}img{max-width:100%}pre{overflow:auto}#viewer{min-width:0;width:100%}</style></head><body><div id="viewer"></div><script id="document-props" type="application/json">${json}</script><script type="module">${script}</script></body></html>`;
}

export async function generateDocumentViewer(files, title) {
  if (files.some(file => file.name.toLowerCase() === "index.html")) return null;
  const documents = files.filter(file => documentTypes.test(file.name));
  if (!documents.length || !process.env.GGUI_MCP_URL) return null;
  if (!process.env.GGUI_MCP_TOKEN) throw new Error("GGUI_MCP_TOKEN is required");
  const timeout = Number(process.env.GGUI_TIMEOUT_MS || 180000);
  if (!Number.isFinite(timeout) || timeout <= 0) throw new Error("GGUI_TIMEOUT_MS must be positive");
  const signal = AbortSignal.timeout(timeout);
  let session;
  let sequence = 0;
  async function rpc(method, params, notify = false) {
    const id = ++sequence;
    const response = await fetch(process.env.GGUI_MCP_URL, { method: "POST", redirect: "error", signal,
      headers: { Authorization: `Bearer ${process.env.GGUI_MCP_TOKEN}`, "Content-Type": "application/json", Accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2025-06-18", ...(session ? { "Mcp-Session-Id": session } : {}) },
      body: JSON.stringify({ jsonrpc: "2.0", ...(!notify ? { id } : {}), method, params }),
    });
    if (!response.ok) throw new Error(`GGUI ${method} failed (${response.status})`);
    session ||= response.headers.get("mcp-session-id");
    if (notify) { await response.body?.cancel(); return; }
    let message;
    if (response.headers.get("content-type")?.includes("text/event-stream")) {
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      try {
        while (!message) {
          const { value, done } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true }).replaceAll("\r\n", "\n");
          let boundary;
          while ((boundary = buffer.indexOf("\n\n")) !== -1) {
            const event = buffer.slice(0, boundary); buffer = buffer.slice(boundary + 2);
            const data = event.split("\n").filter(line => line.startsWith("data:")).map(line => line.slice(5).trimStart()).join("\n");
            if (data) { const item = JSON.parse(data); if (item.id === id) message = item; }
          }
        }
      } finally { await reader.cancel(); }
    } else message = await response.json();
    if (!message || message.id !== id || message.error) throw new Error(`Invalid GGUI ${method} response`);
    if (message.result?.isError) throw new Error(`GGUI ${method} returned a tool error`);
    return message.result;
  }
  const props = { title, documents: (await prepareDocuments(documents)).map(document => ({ html: "", sheets: [], ...document })) };
  await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "evidence-gallery", version: "1.0" } });
  await rpc("notifications/initialized", {}, true);
  const handshake = await rpc("tools/call", { name: "ggui_handshake", arguments: { intent, blueprintDraft: { contract, variance: { seedPrompt: "Document reader v1. Markdown MUST render with native <div dangerouslySetInnerHTML={{__html: document.html}}> (not Box or any design primitive). PDF uses native iframe. Spreadsheet rows[0] is header; body is rows.slice(1), search only body. Each workbook has sheet selection tabs, showing one sheet at a time. All controls local state only. Keep document tabs and download. Fit 390px and desktop widths." } } } });
  const handshakeId = handshake.structuredContent?.handshakeId;
  if (!handshakeId || handshake.structuredContent.action === "declined") throw new Error("GGUI declined document viewer contract");
  const rendered = await rpc("tools/call", { name: "ggui_render", arguments: { handshakeId, props } });
  const { sessionId, outcome } = rendered.structuredContent || {};
  if (!sessionId || outcome !== "rendered") throw new Error("GGUI document viewer generation failed");
  const encoded = rendered._meta?.["ai.ggui/render"]?.codeB64;
  if (typeof encoded !== "string" || !encoded) throw new Error("GGUI did not return compiled viewer code");
  const code = Buffer.from(encoded, "base64").toString("utf8");
  return { html: await bundleViewer(code, props, "js"), sessionId, documents: documents.map(file => file.name) };
}
