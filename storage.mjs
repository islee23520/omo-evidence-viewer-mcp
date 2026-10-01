import { mkdir, readdir, rename, rm, writeFile, readFile, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, resolve, relative, isAbsolute } from "node:path";
import { timingSafeEqual, randomUUID } from "node:crypto";
import { githubRepository } from "./repository.mjs";
import { generateDocumentViewer } from "./ggui.mjs";

export const evidenceRoot = resolve(process.env.EVIDENCE_ROOT || resolve(homedir(), ".omo/evidence/gallery-public"));
export async function reviewResponse(request, root, publicOrigin = process.env.REVIEW_ORIGIN) {
  const url = new URL(request.url);
  const slug = url.searchParams.get('slug');
  if (!slug || !/^[a-z0-9-]+$/.test(slug)) return Response.json({ error: 'invalid slug' }, { status: 400 });
  let entries;
  try {
    const directory = await realpath(resolve(root, slug));
    const inside = relative(await realpath(root), directory);
    if (inside.startsWith('..') || isAbsolute(inside)) throw new Error('outside root');
    entries = JSON.parse(await readFile(resolve(directory, 'manifest.json'), 'utf8'));
    if (!Array.isArray(entries) || entries.some(entry => !entry || !/^[a-f0-9]{64}$/.test(entry.sha256) || typeof entry.name !== 'string')) throw new Error('invalid manifest');
  } catch { return Response.json({ error: 'review manifest not found' }, { status: 404 }); }
  const dir = resolve(root, '.reviews', slug);
  const allowed = new Map(entries.map(entry => [entry.sha256, entry]));
  if (request.method === 'GET') {
    const decisions = {};
    for (const hash of allowed.keys()) {
      try { decisions[hash] = JSON.parse(await readFile(resolve(dir, `${hash}.json`), 'utf8')); }
      catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
    return Response.json({ schemaVersion: 1, decisions }, { headers: { 'Cache-Control': 'no-store' } });
  }
  if (request.method !== 'POST') return new Response('Method not allowed', { status: 405 });
  if (request.headers.get('origin') !== (publicOrigin || url.origin)) return Response.json({ error: 'same-origin review required' }, { status: 403 });
  const text = await request.text();
  if (text.length > 12000) return Response.json({ error: 'review too large' }, { status: 413 });
  let data;
  try { data = JSON.parse(text); } catch { return Response.json({ error: 'invalid JSON' }, { status: 400 }); }
  if (!allowed.has(data.sha256) || !['pass', 'fail', 'pending'].includes(data.verdict) || typeof data.note !== 'string' || data.note.length > 4000) return Response.json({ error: 'invalid image or verdict' }, { status: 400 });
  const decision = { verdict: data.verdict, note: data.note, name: allowed.get(data.sha256).name, updatedAt: new Date().toISOString() };
  await mkdir(dir, { recursive: true });
  const temp = resolve(dir, `${data.sha256}-${randomUUID()}.tmp`);
  await writeFile(temp, JSON.stringify(decision));
  await rename(temp, resolve(dir, `${data.sha256}.json`));
  return Response.json({ sha256: data.sha256, decision, saved: true }, { headers: { 'Cache-Control': 'no-store' } });
}
export const retentionDays = Number(process.env.RETENTION_DAYS || 30);
if (!Number.isFinite(retentionDays) || retentionDays <= 0) throw new Error("RETENTION_DAYS must be positive");

export async function cleanupEvidence(now = Date.now()) {
  await mkdir(evidenceRoot, { recursive: true });
  const removed = [];
  for (const entry of await readdir(evidenceRoot, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name.startsWith(".")) continue;
    const folder = resolve(evidenceRoot, entry.name);
    const metadata = Bun.file(resolve(folder, ".gallery.json"));
    if (!(await metadata.exists())) continue;
    const record = await metadata.json();
    const uploaded = Date.parse(record.uploadedAt);
    if (!Number.isFinite(uploaded) || uploaded >= now - retentionDays * 86400000) continue;
    await rm(folder, { recursive: true });
    removed.push(entry.name);
  }
  return removed;
}

export async function uploadEvidence(request, existingSlug) {
  const token = process.env.UPLOAD_TOKEN;
  if (!token) return Response.json({ error: "Uploads are disabled" }, { status: 503 });
  const supplied = Buffer.from(request.headers.get("authorization") || "");
  const expected = Buffer.from(`Bearer ${token}`);
  if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected)) {
    return Response.json({ error: "Unauthorized" }, { status: 401 });
  }
  if (existingSlug !== undefined) {
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(existingSlug)) return Response.json({ error: "Invalid slug" }, { status: 400 });
    let visibility;
    try { ({ visibility } = await request.json()); } catch { return Response.json({ error: "Invalid JSON" }, { status: 400 }); }
    if (!["private", "public"].includes(visibility)) return Response.json({ error: "Visibility must be private or public" }, { status: 400 });
    const path = resolve(evidenceRoot, existingSlug, ".gallery.json");
    if (!(await Bun.file(path).exists())) return Response.json({ error: "Not found" }, { status: 404 });
    const record = await Bun.file(path).json();
    record.visibility = visibility;
    record.updatedAt = new Date().toISOString();
    await writeFile(path, JSON.stringify(record, null, 2) + "\n");
    return Response.json({ slug: existingSlug, url: `${visibility === "public" ? "/public/" : "/evidence/"}${existingSlug}/`, ...record }, { headers: { "Cache-Control": "no-store" } });
  }
  let form, metadata;
  try {
    form = await request.formData();
    metadata = JSON.parse(form.get("metadata"));
  } catch {
    return Response.json({ error: "Expected multipart files and JSON metadata" }, { status: 400 });
  }
  const { slug, title, repository, labels = [], categories = [], visibility = "private" } = metadata ?? {};
  if (!["private", "public"].includes(visibility)) return Response.json({ error: "Visibility must be private or public" }, { status: 400 });
  let canonicalRepository;
  try { canonicalRepository = githubRepository(repository); } catch (error) { return Response.json({ error: error.message }, { status: 400 }); }
  const files = form.getAll("files");
  if (typeof slug !== "string" || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug) || slug.length > 80 ||
      typeof title !== "string" || !title.trim() || title.length > 160 ||
      typeof repository !== "string" || repository.length > 300 || !/^https:\/\/[^\s/@]+\/[^\s]+\/.+/.test(repository) ||
      ![labels, categories].every(tags => Array.isArray(tags) && tags.length <= 20 && tags.every(tag => typeof tag === "string" && tag.trim() && tag.length <= 40)) ||
      !files.length || files.length > 200 || files.some(file => !(file instanceof File) || !file.size)) {
    return Response.json({ error: "Invalid evidence metadata or files" }, { status: 400 });
  }
  const names = files.map(file => file.name);
  const canonicalNames = names.map(name => name.toLowerCase());
  if (new Set(canonicalNames).size !== names.length || names.some(name => name.length > 240 || name.split("/").some(part =>
      !part || part.startsWith(".") || /[\\:<>"|?*\x00-\x1f]/.test(part) || /[. ]$/.test(part) || /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(part))) ||
      canonicalNames.some(name => canonicalNames.some(other => other.startsWith(`${name}/`)))) {
    return Response.json({ error: "Unsafe or conflicting file paths" }, { status: 400 });
  }
  const id = `${slug}-${crypto.randomUUID()}`;
  const staging = resolve(evidenceRoot, `.upload-${id}`);
  await mkdir(staging, { recursive: true });
  try {
    for (const file of files) {
      const destination = resolve(staging, file.name);
      await mkdir(dirname(destination), { recursive: true });
      await writeFile(destination, Buffer.from(await file.arrayBuffer()), { flag: "wx" });
    }
    let viewer;
    try {
      viewer = await generateDocumentViewer(files, title.trim());
      if (viewer) await writeFile(resolve(staging, "index.html"), viewer.html, { flag: "wx" });
    } catch (error) {
      console.error("Document viewer generation failed", error);
      return Response.json({ error: "Document viewer generation failed; evidence was not published" }, { status: 502 });
    }
    const timestamp = new Date().toISOString();
    const record = { title: title.trim(), repository: canonicalRepository, visibility,
      ...(viewer ? { documentViewer: { provider: "ggui", sessionId: viewer.sessionId, files: viewer.documents } } : {}),
      date: timestamp.slice(0, 10), createdAt: timestamp, updatedAt: timestamp, uploadedAt: timestamp,
      tags: [...labels.map(value => ({ type: "label", value: value.trim() })), ...categories.map(value => ({ type: "category", value: value.trim() }))] };
    await writeFile(resolve(staging, ".gallery.json"), JSON.stringify(record, null, 2) + "\n");
    await rename(staging, resolve(evidenceRoot, id));
    return Response.json({ slug: id, url: `${visibility === "public" ? "/public/" : "/evidence/"}${id}/`, ...record }, { status: 201 });
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

if (import.meta.main) console.log(JSON.stringify({ removed: await cleanupEvidence(), retentionDays }));
