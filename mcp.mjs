import { lstat, readdir, realpath, stat, writeFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { isAbsolute, relative, resolve } from "node:path";

const root = resolve(process.env.HOME, ".omo/evidence/gallery-public");
const slugPattern = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const tool = {
  name: "register_evidence",
  title: "Register public gallery evidence",
  description: "Label an existing, reviewed folder in gallery-public with a project, date, and title. Does not copy private files. The folder becomes public through the gallery HTTP server.",
  inputSchema: {
    type: "object",
    properties: {
      slug: { type: "string", description: "Existing folder name in ~/.omo/evidence/gallery-public (lowercase letters, digits, hyphens)." },
      project: { type: "string", description: "Project name shown on the gallery home page." },
      date: { type: "string", description: "Publication date in YYYY-MM-DD format." },
      title: { type: "string", description: "Entry title shown in the gallery list." },
    },
    required: ["slug", "project", "date", "title"],
    additionalProperties: false,
  },
};

async function register(args) {
  const { slug, project, date, title } = args ?? {};
  if (typeof slug !== "string" || !slugPattern.test(slug)) throw new Error("Invalid slug");
  if (typeof project !== "string" || !project.trim() || project.length > 100 || typeof title !== "string" || !title.trim() || title.length > 160) throw new Error("Project and title are required and must be short text");
  if (typeof date !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(date) || Number.isNaN(Date.parse(date)) || new Date(date).toISOString().slice(0, 10) !== date) throw new Error("Invalid date; use YYYY-MM-DD");
  const folder = resolve(root, slug);
  const actualRoot = await realpath(root);
  const actual = await realpath(folder);
  const inside = relative(actualRoot, actual);
  if (!inside || inside.startsWith("..") || isAbsolute(inside) || !(await stat(actual)).isDirectory()) throw new Error("Folder must be inside gallery-public");
  if ((await readdir(actual)).filter(name => !name.startsWith(".")).length === 0) throw new Error("Folder has no public evidence");
  const metadata = resolve(actual, ".gallery.json");
  try {
    if (!(await lstat(metadata)).isFile()) throw new Error("Registration metadata is not a regular file");
  } catch (cause) {
    if (cause.code !== "ENOENT") throw cause;
  }
  await writeFile(metadata, JSON.stringify({ project: project.trim(), date, title: title.trim() }, null, 2) + "\n", { flag: "w" });
  return { url: `/evidence/${encodeURIComponent(slug)}/`, project: project.trim(), date, title: title.trim() };
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
