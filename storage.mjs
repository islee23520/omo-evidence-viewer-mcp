import { mkdir, readdir, rename, rm, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, resolve } from "node:path";
import { timingSafeEqual } from "node:crypto";

export const evidenceRoot = resolve(process.env.EVIDENCE_ROOT || resolve(homedir(), ".omo/evidence/gallery-public"));
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

export async function uploadEvidence(request) {
  const token = process.env.UPLOAD_TOKEN;
  if (!token) return Response.json({ error: "Uploads are disabled" }, { status: 503 });
  const supplied = Buffer.from(request.headers.get("authorization") || "");
  const expected = Buffer.from(`Bearer ${token}`);
  if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected)) {
    return Response.json({ error: "Unauthorized" }, { status: 401 });
  }
  let form, metadata;
  try {
    form = await request.formData();
    metadata = JSON.parse(form.get("metadata"));
  } catch {
    return Response.json({ error: "Expected multipart files and JSON metadata" }, { status: 400 });
  }
  const { slug, title, repository, labels = [], categories = [] } = metadata ?? {};
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
    const timestamp = new Date().toISOString();
    const record = { title: title.trim(), repository: repository.replace(/\.git\/?$/, "").replace(/\/$/, "").toLowerCase(),
      date: timestamp.slice(0, 10), createdAt: timestamp, updatedAt: timestamp, uploadedAt: timestamp,
      tags: [...labels.map(value => ({ type: "label", value: value.trim() })), ...categories.map(value => ({ type: "category", value: value.trim() }))] };
    await writeFile(resolve(staging, ".gallery.json"), JSON.stringify(record, null, 2) + "\n");
    await rename(staging, resolve(evidenceRoot, id));
    return Response.json({ slug: id, url: `/evidence/${id}/`, ...record }, { status: 201 });
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

if (import.meta.main) console.log(JSON.stringify({ removed: await cleanupEvidence(), retentionDays }));
