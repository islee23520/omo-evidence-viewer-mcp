import { lstat, readdir, realpath, stat } from "node:fs/promises";
import { basename, extname, isAbsolute, relative, resolve } from "node:path";

const home = process.env.HOME;
const evidenceRoot = resolve(home, ".omo/evidence/gallery-public");
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

function sendFile(path) {
  return new Response(Bun.file(path), { headers: { "Content-Type": types[extname(path).toLowerCase()] || "application/octet-stream", "X-Content-Type-Options": "nosniff" } });
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
    return {
      project: typeof metadata.project === "string" && metadata.project ? metadata.project : (prefixCounts.get(prefix) > 1 ? prefix : entry.name.split("-")[0]),
      date: typeof metadata.date === "string" && metadata.date ? metadata.date : fallbackDate(entry.name, info.mtime),
      title: typeof metadata.title === "string" && metadata.title ? metadata.title : entry.name,
      href: `/evidence/${encodeURIComponent(entry.name)}/`,
      kind: "Evidence",
    };
  }));
  for (const item of [...previews, companyLayout, companyDeckPdf].filter(Boolean)) {
    const source = item.source || item.root;
    const info = source ? await stat(source).catch(() => null) : null;
    if (!info) continue;
    const modified = info.mtime;
    items.push({ project: item.project || item.slug.split("-")[0], date: item.date || fallbackDate(item.slug, modified), title: item.title, href: `/preview/${encodeURIComponent(item.slug)}/`, kind: "Preview" });
  }
  const projects = Map.groupBy(items, item => item.project);
  const sections = [...projects].sort(([a], [b]) => a.localeCompare(b)).map(([project, values]) => {
    const dates = Map.groupBy(values, item => item.date);
    const lists = [...dates].sort(([a], [b]) => b.localeCompare(a)).map(([date, rows]) => `<section class="date"><h3>${escapeHtml(date)}</h3><ul>${rows.sort((a, b) => a.title.localeCompare(b.title)).map(item => `<li><a href="${item.href}">${escapeHtml(item.title)}</a><span>${item.kind}</span></li>`).join("")}</ul></section>`).join("");
    return `<section class="project"><h2>${escapeHtml(project)}</h2>${lists}</section>`;
  }).join("");
  return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>OmO Evidence Gallery</title><style>:root{color-scheme:light}body{font:16px/1.55 system-ui;margin:0;background:#f5f3f9;color:#282044}main{max-width:960px;margin:auto;padding:48px 24px 96px}header{border-bottom:1px solid #d9d3e5;padding-bottom:24px;margin-bottom:36px}h1{font-size:clamp(2rem,5vw,3rem);letter-spacing:-.04em;margin:0 0 8px}p{color:#625a73;margin:0;max-width:65ch}h2{font-size:1.45rem;margin:0 0 16px}h3{font-size:.9rem;color:#675b7e;margin:0 0 8px}.project{background:#fff;border:1px solid #e7e1ef;border-radius:16px;padding:24px;margin:20px 0;box-shadow:0 8px 24px #2820440a}.date+ .date{margin-top:24px}ul{list-style:none;padding:0;margin:0}li{display:flex;align-items:baseline;justify-content:space-between;gap:16px;padding:10px 0;border-top:1px solid #eeeaf3;overflow-wrap:anywhere}a{color:#5336a5;text-decoration:none;font-weight:550}a:hover{text-decoration:underline}a:focus-visible{outline:2px solid #5336a5;outline-offset:3px}span{color:#746b84;font-size:.8rem;white-space:nowrap}@media(max-width:600px){main{padding:32px 16px 64px}.project{padding:18px}li{align-items:start}}</style><main><header><h1>OmO Evidence Gallery</h1><p>검토용 공개 증거를 프로젝트와 날짜별로 모았습니다. 이 서버는 인증 기능이 없으므로 공개 가능한 자료만 등록하세요.</p></header>${sections || "<p>아직 등록된 증거가 없습니다.</p>"}</main></html>`, { headers: { "Content-Type": "text/html; charset=utf-8", "X-Content-Type-Options": "nosniff" } });
}

async function evidencePage(parts) {
  if (parts.some(part => !part || part === "." || part === ".." || part.startsWith("."))) return new Response("Not found", { status: 404 });
  const root = resolve(evidenceRoot, ...parts);
  const rel = relative(evidenceRoot, root);
  if (rel.startsWith("..") || isAbsolute(rel)) return new Response("Not found", { status: 404 });
  try {
    const real = await realpath(root);
    const inside = relative(await realpath(evidenceRoot), real);
    if (inside.startsWith("..") || isAbsolute(inside)) return new Response("Not found", { status: 404 });
    if ((await stat(real)).isFile()) return sendFile(real);
    const entries = await readdir(real, { withFileTypes: true });
    const links = entries.filter(x => !x.name.startsWith(".") && (x.isDirectory() || x.isFile())).map(x => `<li><a href="${encodeURIComponent(x.name)}${x.isDirectory() ? "/" : ""}">${escapeHtml(x.name)}${x.isDirectory() ? "/" : ""}</a></li>`).join("");
    return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><title>${escapeHtml(basename(real))}</title><style>body{font:16px/1.6 system-ui;max-width:900px;margin:5vh auto;padding:0 24px}a{color:#6542c5}li{margin:8px 0}</style><h1>${escapeHtml(basename(real))}</h1><a href="/">갤러리 홈</a><ul>${links}</ul></html>`, { headers: { "Content-Type": "text/html; charset=utf-8", "X-Content-Type-Options": "nosniff" } });
  } catch {
    return new Response("Not found", { status: 404 });
  }
}

const server = Bun.serve({ hostname: host, port, async fetch(request) {
  const path = new URL(request.url).pathname;
  if (path === "/") return indexPage();
  if (path.startsWith("/evidence/")) return evidencePage(path.slice(10).split("/").filter(Boolean).map(decodeURIComponent));
  const companyPath = companyLayout && `/preview/${companyLayout.slug}/`;
  if (path === companyPath) {
    const images = companyLayout.images.map((image, i) => `<figure><figcaption>${i === 0 ? "데스크톱 · 1440px" : "모바일 · 390px"}</figcaption><a href="${image}"><img src="${image}" alt="리나랩 COMPANY ${i === 0 ? "데스크톱" : "모바일"} 레이아웃" loading="lazy"></a></figure>`).join("");
    return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${companyLayout.title}</title><style>body{font:16px/1.6 system-ui;margin:0;background:#101010;color:#fff}main{max-width:1488px;margin:auto;padding:32px 24px 80px}a{color:inherit}h1{font-size:clamp(1.7rem,4vw,3rem);line-height:1.2}p,figcaption{color:#aaa}figure{margin:44px 0}img{display:block;width:100%;height:auto;border:1px solid #444}figure:nth-of-type(2){max-width:390px}</style><main><a href="/">← 갤러리</a><h1>${companyLayout.title}</h1><p>기존 색상·타이포·문구를 유지하고 COMPANY 덱의 배치만 바꾼 화면입니다.</p>${images}</main></html>`, { headers: { "Content-Type": types[".html"], "X-Content-Type-Options": "nosniff" } });
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
    const slides = companyDeckPdf.pages.map((page, index) => `<figure><figcaption>${String(index + 1).padStart(2, "0")} / ${String(companyDeckPdf.pages.length).padStart(2, "0")}</figcaption><img src="${page}" alt="리나랩 COMPANY 덱 ${index + 1}쪽"></figure>`).join("");
    return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${companyDeckPdf.title}</title><style>body{margin:0;background:#080808;color:#f6f6f6;font:16px/1.55 system-ui}main{max-width:1200px;margin:auto;padding:32px 24px 80px}a{color:inherit}h1{font-size:clamp(1.8rem,4vw,3rem)}p,figcaption{color:#b7b7b7}figure{margin:42px 0}img{display:block;width:100%;height:auto;border:1px solid #505050}</style><main><a href="/">← 갤러리</a><h1>${companyDeckPdf.title}</h1><p>원본 덱의 구성을 바탕으로 현재 공식 프로젝트 내용으로 다시 제작했습니다.</p><p><a href="${companyDeckPdf.pdf}">PDF 다운로드 · ${companyDeckPdf.pages.length}쪽</a></p>${slides}</main></html>`, { headers: { "Content-Type": types[".html"], "X-Content-Type-Options": "nosniff" } });
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
