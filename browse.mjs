import { homedir } from "node:os";
import { resolve } from "node:path";

const configPath = process.env.EVIDENCE_CLIENT_CONFIG || resolve(homedir(), ".omo/evidence-client.json");
const config = await Bun.file(configPath).json();
const upstream = new URL(config.EVIDENCE_SERVER_URL);
if (upstream.protocol !== "https:") throw new Error("Evidence browsing requires an HTTPS upstream");
if (!config.CF_ACCESS_CLIENT_ID || !config.CF_ACCESS_CLIENT_SECRET) throw new Error("Evidence browsing requires service credentials");
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: Number(process.env.EVIDENCE_BROWSER_PORT || 17677),
  async fetch(request) {
    if (!["GET", "HEAD"].includes(request.method)) return new Response("Read-only browser gateway", { status: 405 });
    const incoming = new URL(request.url);
    const target = new URL(incoming.pathname + incoming.search, upstream);
    const headers = new Headers();
    headers.set("CF-Access-Client-Id", config.CF_ACCESS_CLIENT_ID);
    headers.set("CF-Access-Client-Secret", config.CF_ACCESS_CLIENT_SECRET);
    for (const name of ["range", "if-none-match", "if-modified-since", "accept"]) {
      if (request.headers.has(name)) headers.set(name, request.headers.get(name));
    }
    const response = await fetch(target, { method: request.method, headers, redirect: "manual" });
    const returned = new Headers(response.headers);
    returned.delete("set-cookie");
    returned.delete("content-encoding");
    returned.delete("content-length");
    const location = returned.get("location");
    if (location) {
      const redirect = new URL(location, upstream);
      if (redirect.origin === upstream.origin) returned.set("location", redirect.pathname + redirect.search);
    }
    return new Response(request.method === "HEAD" ? null : response.body, { status: response.status, headers: returned });
  },
});
console.log(`Evidence browser gateway: ${server.url}`);
