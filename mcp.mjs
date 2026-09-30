import { lstat, readdir, realpath, stat, writeFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { isAbsolute, relative, resolve } from "node:path";
import { homedir } from "node:os";

const clientConfigPath = process.env.EVIDENCE_CLIENT_CONFIG || resolve(homedir(), ".omo/evidence-client.json");
if (await Bun.file(clientConfigPath).exists()) {
  const config = await Bun.file(clientConfigPath).json();
  for (const key of ["EVIDENCE_SERVER_URL", "UPLOAD_TOKEN", "CF_ACCESS_CLIENT_ID", "CF_ACCESS_CLIENT_SECRET", "EVIDENCE_ROOT"]) {
    if (process.env[key] === undefined && typeof config[key] === "string") process.env[key] = config[key];
  }
}
const root = resolve(process.env.EVIDENCE_ROOT || resolve(homedir(), ".omo/evidence/gallery-public"));
const slugPattern = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const tool = {
  name: "register_evidence",
  title: "Register public gallery evidence",
  description: "Publish a reviewed staging folder to the configured remote gallery, or register it locally when no remote is configured. Never include private files.",
  inputSchema: {
    type: "object",
    properties: {
      slug: { type: "string", description: "Existing folder name in ~/.omo/evidence/gallery-public (lowercase letters, digits, hyphens)." },
      repository: { type: "string", description: "Git remote URL (HTTPS or SSH), used to group evidence by repository." },
      project: { type: "string", description: "Legacy project name; ignored when repository is provided." },
      date: { type: "string", description: "Optional publication date in YYYY-MM-DD format; defaults to today." },
      title: { type: "string", description: "Entry title shown in the gallery list." },
      labels: { type: "array", items: { type: "string" }, description: "Label tags for this entry." },
      categories: { type: "array", items: { type: "string" }, description: "Category tags for this entry." },
    },
    required: ["slug", "repository", "title", "labels", "categories"],
    additionalProperties: false,
  },
};

async function register(args) {
  const { slug, repository, project, date, title, labels = [], categories = [] } = args ?? {};
  if (typeof slug !== "string" || !slugPattern.test(slug)) throw new Error("Invalid slug");
  if (typeof title !== "string" || !title.trim() || title.length > 160) throw new Error("Title is required and must be short text");
  if (repository !== undefined && (typeof repository !== "string" || !/^(?:https:\/\/[^\s/@]+(?:\/[^\s]+)+|git@[^\s:]+:[^\s/]+\/[^\s]+)$/.test(repository) || repository.length > 300)) throw new Error("Repository must be a Git HTTPS or SSH remote URL");
  if (!repository && (typeof project !== "string" || !project.trim() || project.length > 100)) throw new Error("Repository is required");
  const publicationDate = date ?? new Date().toISOString().slice(0, 10);
  if (typeof publicationDate !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(publicationDate) || Number.isNaN(Date.parse(publicationDate)) || new Date(publicationDate).toISOString().slice(0, 10) !== publicationDate) throw new Error("Invalid date; use YYYY-MM-DD");
  if (![labels, categories].every(values => Array.isArray(values) && values.length <= 20 && values.every(value => typeof value === "string" && value.trim() && value.length <= 40))) throw new Error("Labels and categories must be arrays of short, nonempty tags");
  const canonicalRepository = repository?.trim().replace(/^git@([^:]+):/, "https://$1/").replace(/\.git\/?$/, "").replace(/\/$/, "").toLowerCase();
  const tags = [...labels.map(value => ({ type: "label", value: value.trim() })), ...categories.map(value => ({ type: "category", value: value.trim() }))];
  const folder = resolve(root, slug);
  const actualRoot = await realpath(root);
  const actual = await realpath(folder);
  const inside = relative(actualRoot, actual);
  if (!inside || inside.startsWith("..") || isAbsolute(inside) || !(await stat(actual)).isDirectory()) throw new Error("Folder must be inside gallery-public");
  if ((await readdir(actual)).filter(name => !name.startsWith(".")).length === 0) throw new Error("Folder has no public evidence");
  const metadata = resolve(actual, ".gallery.json");
  let createdAt;
  try {
    if (!(await lstat(metadata)).isFile()) throw new Error("Registration metadata is not a regular file");
    createdAt = (await Bun.file(metadata).json()).createdAt;
  } catch (cause) {
    if (cause.code !== "ENOENT") throw cause;
  }
  const record = { ...(canonicalRepository ? { repository: canonicalRepository } : { project: project.trim() }), date: publicationDate, title: title.trim(), tags, createdAt: createdAt || new Date().toISOString(), updatedAt: new Date().toISOString() };
  if (process.env.EVIDENCE_SERVER_URL) {
    if (!canonicalRepository) throw new Error("Remote publication requires a repository URL");
    if (!process.env.UPLOAD_TOKEN) throw new Error("Remote publication requires UPLOAD_TOKEN");
    const base = new URL(process.env.EVIDENCE_SERVER_URL);
    if (base.protocol !== "https:" && !["localhost", "127.0.0.1"].includes(base.hostname)) throw new Error("Remote publication requires HTTPS");
    const headers = { Authorization: `Bearer ${process.env.UPLOAD_TOKEN}` };
    if (process.env.CF_ACCESS_CLIENT_ID && process.env.CF_ACCESS_CLIENT_SECRET) {
      headers["CF-Access-Client-Id"] = process.env.CF_ACCESS_CLIENT_ID;
      headers["CF-Access-Client-Secret"] = process.env.CF_ACCESS_CLIENT_SECRET;
    }
    const form = new FormData();
    form.set("metadata", JSON.stringify({ slug, repository: canonicalRepository, title, labels, categories }));
    const assets = [];
    async function collect(directory, prefix = "") {
      for (const entry of await readdir(directory, { withFileTypes: true })) {
        if (entry.name.startsWith(".")) continue;
        const path = resolve(directory, entry.name);
        const name = prefix + entry.name;
        if (entry.isDirectory()) await collect(path, name + "/");
        else if (entry.isFile()) {
          const file = Bun.file(path);
          form.append("files", file, name);
          assets.push({ name, hash: Bun.SHA256.hash(await file.arrayBuffer(), "hex") });
        } else throw new Error("Publication does not accept symlinks or special files");
      }
    }
    await collect(actual);
    const response = await fetch(new URL("/api/evidence", base), { method: "POST", headers, body: form, redirect: "error" });
    if (!response.ok) throw new Error(`Remote upload failed (${response.status})`);
    const published = await response.json();
    const url = new URL(published.url, base);
    if (url.origin !== base.origin || !url.pathname.startsWith("/evidence/")) throw new Error("Invalid remote evidence URL");
    for (const asset of assets) {
      const fetched = await fetch(new URL(asset.name.split("/").map(encodeURIComponent).join("/"), url), { headers, redirect: "error" });
      if (!fetched.ok || Bun.SHA256.hash(await fetched.arrayBuffer(), "hex") !== asset.hash) throw new Error(`Remote verification failed: ${asset.name}`);
    }
    return { ...published, url: url.href, verifiedFiles: assets.length };
  }
  await writeFile(metadata, JSON.stringify(record, null, 2) + "\n", { flag: "w" });
  return { url: `/evidence/${encodeURIComponent(slug)}/`, ...record };
}

const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
let initialized = false;
for await (const line of input) {
  let message;
  try { message = JSON.parse(line); } catch { console.log(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } })); continue; }
  if (message.method === "notifications/initialized") { initialized = true; continue; }
  if (message.id === undefined) continue;
  let result;
  let error;
  if (message.method === "initialize") {
    result = { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "omo-evidence-gallery", version: "1.0.0" } };
  } else if (!initialized) {
    error = { code: -32000, message: "Initialize first" };
  } else if (message.method === "tools/list") {
    result = { tools: [tool] };
  } else if (message.method === "tools/call" && message.params?.name === tool.name) {
    try {
      const registered = await register(message.params.arguments);
      result = { content: [{ type: "text", text: JSON.stringify(registered) }], structuredContent: registered };
    } catch (cause) {
      result = { content: [{ type: "text", text: cause.message }], isError: true };
    }
  } else {
    error = { code: -32601, message: "Method not found" };
  }
  console.log(JSON.stringify({ jsonrpc: "2.0", id: message.id, ...(error ? { error } : { result }) }));
}
