import { lstat, readdir, realpath, stat } from "node:fs/promises";
import { basename, extname, isAbsolute, relative, resolve } from "node:path";
import { cleanupEvidence, evidenceRoot, uploadEvidence, reviewResponse } from "./storage.mjs";
import { githubRepository } from "./repository.mjs";

const port = Number(process.env.PORT || 17678);
const host = process.env.HOST || "0.0.0.0";
const configPath = resolve(import.meta.dir, "local-previews.json");
const local = await Bun.file(configPath).exists() ? await Bun.file(configPath).json() : {};
const previews = (local.previews || []).map(item => ({ ...item, allowedAssets: new Set(item.allowedAssets) }));
const companyLayout = local.companyLayout;
const companyDeckPdf = local.companyDeckPdf;

const types = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".mp4": "video/mp4",
  ".webm": "video/webm",
  ".mov": "video/quicktime",
  ".m4v": "video/x-m4v",
  ".pdf": "application/pdf",
  ".txt": "text/plain; charset=utf-8",
  ".md": "text/plain; charset=utf-8",
};

function escapeHtml(value) {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

async function fileInside(root, parts) {
  if (!parts.length || parts.some(part => !part || part === "." || part === ".." || part.startsWith("."))) return null;
  const candidate = resolve(root, ...parts);
  const rel = relative(root, candidate);
  if (!rel || rel.startsWith("..") || isAbsolute(rel)) return null;
  try {
    const actual = await realpath(candidate);
    const inside = relative(await realpath(root), actual);
    if (!inside || inside.startsWith("..") || isAbsolute(inside) || !(await stat(actual)).isFile()) return null;
    return actual;
  } catch {
    return null;
  }
}

function sendFile(path, raw = false) {
  if (!raw && extname(path).toLowerCase() === ".html") {
    return Bun.file(path).text().then(html => {
      const link = '<a href="/" data-gallery-home style="position:fixed;right:16px;bottom:16px;z-index:2147483647;padding:10px 16px;border-radius:24px;background:#282044;color:white;font:14px system-ui;text-decoration:none;box-shadow:0 2px 12px #0004">갤러리 홈</a>';
      const page = /<body(?:\s|>)/i.test(html) ? html.replace(/<body([^>]*)>/i, `<body$1>${link}`) : html + link;
      return new Response(page, { headers: { "Content-Type": types[".html"], "X-Content-Type-Options": "nosniff" } });
    });
  }
  return new Response(Bun.file(path), { headers: { "Content-Type": types[extname(path).toLowerCase()] || "application/octet-stream", "X-Content-Type-Options": "nosniff" } });
}

const imageTypes = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"]);
const videoTypes = new Set([".mp4", ".webm", ".mov", ".m4v"]);

function mediaViewer(items, title) {
  if (!items.length) return "";
  const data = JSON.stringify(items).replaceAll("<", "\\u003c");
  return `<section class="viewer" aria-label="${escapeHtml(title)} 미디어 탐색"><div class="viewer-bar"><button type="button" id="previous" aria-label="이전 항목">← 이전</button><span id="position" aria-live="polite"></span><button type="button" id="next" aria-label="다음 항목">다음 →</button></div><div id="media"></div><p class="viewer-name"><a id="original" target="_blank" rel="noopener">원본 열기</a></p></section><style>.viewer{margin:24px 0 36px;background:#16141c;color:#f5f3f9;border-radius:12px;padding:16px}.viewer-bar{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-bottom:16px}.viewer-bar span{color:#d7d1e1;text-align:center;overflow-wrap:anywhere}.viewer button{font:inherit;color:inherit;background:#35303e;border:1px solid #625a73;border-radius:8px;padding:8px 14px;cursor:pointer}.viewer button:hover:not(:disabled){background:#514763}.viewer button:disabled{opacity:.45;cursor:default}.viewer button:focus-visible,.viewer a:focus-visible{outline:2px solid #d0baff;outline-offset:3px}.viewer #media{display:grid;place-items:center;min-height:200px}.viewer img,.viewer video,.viewer iframe{display:block;max-width:100%;max-height:75vh;border:0}.viewer video{width:100%}.viewer iframe{width:100%;height:75vh;background:white}.viewer-name{margin:12px 0 0;overflow-wrap:anywhere}.viewer-name a{color:#d0baff}@media(max-width:600px){.viewer{padding:12px}.viewer button{padding:8px}}
  </style><script>(()=>{const items=${data};const media=document.getElementById("media");const position=document.getElementById("position");const previous=document.getElementById("previous");const next=document.getElementById("next");const original=document.getElementById("original");let index=Number(new URL(location.href).searchParams.get("page"))-1;if(!Number.isInteger(index)||index<0||index>=items.length)index=0;function show(i,record=true){index=i;const item=items[i];const element=document.createElement(item.kind==="image"?"img":item.kind==="video"?"video":"iframe");element.src=item.href;if(item.kind==="image")element.alt=item.name;if(item.kind==="video")element.controls=true;if(item.kind==="pdf")element.title=item.name;media.replaceChildren(element);position.textContent=(i+1)+" / "+items.length+" · "+item.name;original.href=item.href;previous.disabled=i===0;next.disabled=i===items.length-1;if(record){const url=new URL(location.href);url.searchParams.set("page",i+1);history.pushState({page:i+1},"",url)}if(items[i+1]?.kind==="image"){const preload=new Image();preload.src=items[i+1].href}}previous.onclick=()=>{if(index>0)show(index-1)};next.onclick=()=>{if(index<items.length-1)show(index+1)};document.addEventListener("keydown",event=>{if(["INPUT","TEXTAREA","SELECT"].includes(document.activeElement.tagName))return;if(event.key==="ArrowLeft"&&index>0)show(index-1);if(event.key==="ArrowRight"&&index<items.length-1)show(index+1)});addEventListener("popstate",()=>{const page=Number(new URL(location.href).searchParams.get("page"))-1;show(Number.isInteger(page)&&page>=0&&page<items.length?page:0,false)});show(index,false)})()</script>`;
}

async function indexPage() {
  const entries = await readdir(evidenceRoot, { withFileTypes: true });
  const folders = entries.filter(entry => entry.isDirectory() && !entry.name.startsWith("."));
  const prefixCounts = new Map();
  for (const entry of folders) {
    const prefix = entry.name.split("-").slice(0, 2).join("-");
    prefixCounts.set(prefix, (prefixCounts.get(prefix) || 0) + 1);
  }
  const fallbackDate = (name, modified) => {
    const match = /(?:^|-)20\d{6}(?=-|$)/.exec(name);
    return match ? `${match[0].slice(-8, -4)}-${match[0].slice(-4, -2)}-${match[0].slice(-2)}` : modified.toISOString().slice(0, 10);
  };
  const items = await Promise.all(folders.map(async entry => {
    const path = resolve(evidenceRoot, entry.name);
    const info = await stat(path);
    let metadata = {};
    const registered = resolve(path, ".gallery.json");
    try {
      if ((await lstat(registered)).isFile()) metadata = await Bun.file(registered).json();
    } catch { /* Older folders or invalid metadata fall back to the folder name. */ }
    const prefix = entry.name.split("-").slice(0, 2).join("-");
    let repository;
    try { repository = githubRepository(metadata.repository); } catch { return null; }
    return {
      project: repository.replace("https://github.com/", ""),
      date: typeof metadata.date === "string" && metadata.date ? metadata.date : fallbackDate(entry.name, info.mtime),
      created: Date.parse(metadata.createdAt) || info.birthtimeMs || info.mtimeMs,
      title: typeof metadata.title === "string" && metadata.title ? metadata.title : entry.name,
      tags: Array.isArray(metadata.tags) ? metadata.tags.filter(tag => tag && ["label", "category"].includes(tag.type) && typeof tag.value === "string") : [],
      href: `/evidence/${encodeURIComponent(entry.name)}/`,
      kind: "Evidence",
    };
  })).then(items => items.filter(Boolean));
  for (const item of [...previews, companyLayout, companyDeckPdf].filter(Boolean)) {
    const source = item.source || item.root;
    const info = source ? await stat(source).catch(() => null) : null;
    if (!info) continue;
    const modified = info.mtime;
    items.push({ project: item.repository ? item.repository.replace(/\.git\/?$/, "").replace(/\/$/, "").split(/[/:]/).at(-1) : item.project || item.slug.split("-")[0], date: item.date || fallbackDate(item.slug, modified), created: info.birthtimeMs || info.mtimeMs, title: item.title, tags: [], href: `/preview/${encodeURIComponent(item.slug)}/`, kind: "Preview" });
  }
  const projects = Map.groupBy(items, item => item.project);
  const sections = [...projects].sort(([a, av], [b, bv]) => Math.max(...bv.map(item => item.created)) - Math.max(...av.map(item => item.created)) || a.localeCompare(b)).map(([project, values]) => {
    const lists = values.sort((a, b) => b.created - a.created || a.title.localeCompare(b.title)).map(item => `<li><div><a href="${item.href}">${escapeHtml(item.title)}</a>${item.tags.length ? `<div class="tags">${item.tags.map(tag => `<span class="tag ${tag.type}">${tag.type === "category" ? "카테고리" : "라벨"} · ${escapeHtml(tag.value)}</span>`).join("")}</div>` : ""}</div><span>${escapeHtml(item.date)} · ${item.kind}</span></li>`).join("");
    return `<section class="project"><h2>${escapeHtml(project)}</h2><ul>${lists}</ul></section>`;
  }).join("");
  return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>OmO Evidence Gallery</title><style>:root{color-scheme:light}body{font:16px/1.55 system-ui;margin:0;background:#f5f3f9;color:#282044}main{max-width:960px;margin:auto;padding:48px 24px 96px}header{border-bottom:1px solid #d9d3e5;padding-bottom:24px;margin-bottom:36px}h1{font-size:clamp(2rem,5vw,3rem);letter-spacing:-.04em;margin:0 0 8px}p{color:#625a73;margin:0;max-width:65ch}h2{font-size:1.45rem;margin:0 0 16px}.project{background:#fff;border:1px solid #e7e1ef;border-radius:16px;padding:24px;margin:20px 0;box-shadow:0 8px 24px #2820440a}ul{list-style:none;padding:0;margin:0}li{display:flex;align-items:baseline;justify-content:space-between;gap:16px;padding:10px 0;border-top:1px solid #eeeaf3;overflow-wrap:anywhere}a{color:#5336a5;text-decoration:none;font-weight:550}a:hover{text-decoration:underline}a:focus-visible{outline:2px solid #5336a5;outline-offset:3px}li>span{color:#746b84;font-size:.8rem;white-space:nowrap}.tags{display:flex;flex-wrap:wrap;gap:6px;margin-top:6px}.tag{font-size:.75rem;color:#5336a5;background:#eee8fa;border-radius:999px;padding:2px 8px}.tag.category{color:#17665b;background:#e1f4ef}@media(max-width:600px){main{padding:32px 16px 64px}.project{padding:18px}li{align-items:start;flex-direction:column;gap:4px}}</style><main><header><h1>OmO Evidence Gallery</h1><p>검토용 공개 증거를 Git 저장소별로 묶고 나중에 만든 항목부터 표시합니다. 자료는 기본 비공개이며 인증된 사용자와 허용된 기기만 볼 수 있습니다. 명시적으로 공개한 자료만 공유 링크로 열립니다.</p></header>${sections || "<p>아직 등록된 증거가 없습니다.</p>"}</main></html>`, { headers: { "Content-Type": "text/html; charset=utf-8", "X-Content-Type-Options": "nosniff" } });
}

async function evidencePage(parts, raw = false) {
  if (parts.some(part => !part || part === "." || part === ".." || part.startsWith("."))) return new Response("Not found", { status: 404 });
  const root = resolve(evidenceRoot, ...parts);
  const rel = relative(evidenceRoot, root);
  if (rel.startsWith("..") || isAbsolute(rel)) return new Response("Not found", { status: 404 });
  try {
    const real = await realpath(root);
    const inside = relative(await realpath(evidenceRoot), real);
    if (inside.startsWith("..") || isAbsolute(inside)) return new Response("Not found", { status: 404 });
    if ((await stat(real)).isFile()) return sendFile(real, raw);
    const entries = await readdir(real, { withFileTypes: true });
    const visible = entries.filter(x => !x.name.startsWith(".") && (x.isDirectory() || x.isFile())).sort((a, b) => a.name.localeCompare(b.name, "en", { numeric: true }));
    const media = visible.filter(x => x.isFile() && (imageTypes.has(extname(x.name).toLowerCase()) || videoTypes.has(extname(x.name).toLowerCase()) || extname(x.name).toLowerCase() === ".pdf")).map(x => ({ name: x.name, href: encodeURIComponent(x.name), kind: imageTypes.has(extname(x.name).toLowerCase()) ? "image" : videoTypes.has(extname(x.name).toLowerCase()) ? "video" : "pdf" }));
    const links = visible.map(x => `<li><a href="${encodeURIComponent(x.name)}${x.isDirectory() ? "/" : ""}">${escapeHtml(x.name)}${x.isDirectory() ? "/" : ""}</a></li>`).join("");
    return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escapeHtml(basename(real))}</title><style>body{font:16px/1.6 system-ui;max-width:900px;margin:5vh auto;padding:0 24px}a{color:#6542c5}li{margin:8px 0;overflow-wrap:anywhere}</style><h1>${escapeHtml(basename(real))}</h1><a href="/">갤러리 홈</a>${mediaViewer(media, basename(real))}<ul>${links}</ul></html>`, { headers: { "Content-Type": "text/html; charset=utf-8", "X-Content-Type-Options": "nosniff" } });
  } catch {
    return new Response("Not found", { status: 404 });
  }
}

await cleanupEvidence();
const cleanupInterval = Number(process.env.CLEANUP_INTERVAL_HOURS || 24);
if (!Number.isFinite(cleanupInterval) || cleanupInterval <= 0) throw new Error("CLEANUP_INTERVAL_HOURS must be positive");
setInterval(() => cleanupEvidence().then(removed => console.log(JSON.stringify({ event: "retention", removed }))).catch(error => console.error("Retention cleanup failed", error)), cleanupInterval * 3600000).unref();

const server = Bun.serve({ hostname: host, port, maxRequestBodySize: 100 * 1024 * 1024, async fetch(request) {
  const path = new URL(request.url).pathname;
  if (path === "/health") return Response.json({ status: "ok" });
  if (path === "/api/reviews") return reviewResponse(request, evidenceRoot);
  if (path === "/api/evidence" && request.method === "POST") return uploadEvidence(request);
  const visibilityPath = /^\/api\/evidence\/([a-z0-9-]+)\/visibility$/.exec(path);
  if (visibilityPath && request.method === "PATCH") return uploadEvidence(request, visibilityPath[1]);
  if (path.startsWith("/public/")) {
    let parts;
    try { parts = path.slice(8).split("/").filter(Boolean).map(decodeURIComponent); } catch { return new Response("Not found", { status: 404 }); }
    if (!parts.length || parts.some(part => !part || part.startsWith(".") || /[\\/]/.test(part))) return new Response("Not found", { status: 404 });
    try {
      const entryRoot = await realpath(resolve(evidenceRoot, parts[0]));
      const publicTarget = await realpath(resolve(entryRoot, ...parts.slice(1)));
      const withinEntry = relative(entryRoot, publicTarget);
      if (withinEntry.startsWith("..") || isAbsolute(withinEntry)) return new Response("Not found", { status: 404 });
      const metadata = await Bun.file(resolve(evidenceRoot, parts[0], ".gallery.json")).json();
      if (metadata.visibility !== "public") return new Response("Not found", { status: 404 });
    } catch { return new Response("Not found", { status: 404 }); }
    const response = await evidencePage(parts, new URL(request.url).searchParams.get("raw") === "1");
    const headers = new Headers(response.headers);
    headers.set("Cache-Control", "no-store");
    return new Response(response.body, { status: response.status, headers });
  }
  if (path === "/") return indexPage();
  if (path.startsWith("/evidence/")) return evidencePage(path.slice(10).split("/").filter(Boolean).map(decodeURIComponent), new URL(request.url).searchParams.get("raw") === "1");
  const companyPath = companyLayout && `/preview/${companyLayout.slug}/`;
  if (path === companyPath) {
    const images = companyLayout.images.map((image, i) => ({ name: i === 0 ? "데스크톱 · 1440px" : "모바일 · 390px", href: image, kind: "image" }));
    return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escapeHtml(companyLayout.title)}</title><style>body{font:16px/1.6 system-ui;margin:0;background:#101010;color:#fff}main{max-width:1488px;margin:auto;padding:32px 24px 80px}a{color:inherit}h1{font-size:clamp(1.7rem,4vw,3rem);line-height:1.2}p{color:#aaa}</style><main><a href="/">← 갤러리</a><h1>${escapeHtml(companyLayout.title)}</h1><p>기존 색상·타이포·문구를 유지하고 COMPANY 덱의 배치만 바꾼 화면입니다.</p>${mediaViewer(images, companyLayout.title)}</main></html>`, { headers: { "Content-Type": types[".html"], "X-Content-Type-Options": "nosniff" } });
  }
  if (companyPath && path.startsWith(companyPath)) {
    const image = path.slice(companyPath.length);
    if (companyLayout.images.includes(image)) {
      const file = await fileInside(companyLayout.root, [image]);
      if (file) return sendFile(file);
    }
    return new Response("Not found", { status: 404 });
  }
  const deckPath = companyDeckPdf && `/preview/${companyDeckPdf.slug}/`;
  if (path === deckPath) {
    const slides = companyDeckPdf.pages.map((page, index) => ({ name: `${index + 1}쪽`, href: page, kind: "image" }));
    return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${escapeHtml(companyDeckPdf.title)}</title><style>body{margin:0;background:#080808;color:#f6f6f6;font:16px/1.55 system-ui}main{max-width:1200px;margin:auto;padding:32px 24px 80px}a{color:inherit}h1{font-size:clamp(1.8rem,4vw,3rem)}p{color:#b7b7b7}</style><main><a href="/">← 갤러리</a><h1>${escapeHtml(companyDeckPdf.title)}</h1><p>원본 덱의 구성을 바탕으로 현재 공식 프로젝트 내용으로 다시 제작했습니다.</p><p><a href="${escapeHtml(companyDeckPdf.pdf)}">PDF 다운로드 · ${companyDeckPdf.pages.length}쪽</a></p>${mediaViewer(slides, companyDeckPdf.title)}</main></html>`, { headers: { "Content-Type": types[".html"], "X-Content-Type-Options": "nosniff" } });
  }
  if (deckPath && path.startsWith(deckPath)) {
    const name = path.slice(deckPath.length);
    if (name === companyDeckPdf.pdf || companyDeckPdf.pages.includes(name)) {
      const parts = name === companyDeckPdf.pdf ? [name] : ["pages", name];
      const file = await fileInside(companyDeckPdf.root, parts);
      if (file) return sendFile(file);
    }
    return new Response("Not found", { status: 404 });
  }
  const match = /^\/preview\/([a-z0-9-]+)\/(.*)$/.exec(path);
  if (match) {
    const preview = previews.find(item => item.slug === match[1]);
    if (!preview) return new Response("Not found", { status: 404 });
    if (!match[2]) {
      if (!(await Bun.file(preview.source).exists())) return new Response("Not found", { status: 404 });
      const html = (await Bun.file(preview.source).text()).replaceAll('../../frontend/shared/assets/mascot/', `/preview/${preview.slug}/assets/`);
      return new Response(html, { headers: { "Content-Type": types[".html"], "X-Content-Type-Options": "nosniff" } });
    }
    const asset = /^assets\/(admin-book-guide|eden-thinking-guide|eden-bitna-guide)\/(still\.png|guide_idle\.gif)$/.exec(match[2]);
    if (asset && preview.allowedAssets.has(asset[1])) {
      const file = await fileInside(preview.assets, [asset[1], asset[2]]);
      if (file) return sendFile(file);
    }
  }
  return new Response("Not found", { status: 404 });
} });
console.log(`OmO evidence gallery listening on ${server.url}`);
