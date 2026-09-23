import { readdir, realpath, stat } from "node:fs/promises";
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
  const dirs = entries.filter(entry => entry.isDirectory() && !entry.name.startsWith(".")).map(entry => `<li><a href="/evidence/${encodeURIComponent(entry.name)}/">${escapeHtml(entry.name)}</a></li>`).join("");
  const demos = [...previews, companyLayout, companyDeckPdf].filter(Boolean).map(item => `<li><a href="/preview/${item.slug}/">${escapeHtml(item.title)}</a></li>`).join("");
  return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>OmO Evidence Gallery</title><style>body{font:16px/1.6 system-ui;max-width:900px;margin:5vh auto;padding:0 24px;background:#faf9ff;color:#282044}a{color:#6542c5}li{margin:8px 0}h1{font-size:2rem}</style><h1>OmO Evidence Gallery</h1><p>검토용 공개 증거만 여기에 등록합니다. 이 서버는 인증 기능이 없으므로 모든 OmO 파일을 게시하지 않습니다.</p><h2>미리보기</h2><ul>${demos}</ul><h2>공개 증거 폴더</h2><ul>${dirs}</ul></html>`, { headers: { "Content-Type": "text/html; charset=utf-8", "X-Content-Type-Options": "nosniff" } });
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
    const slides = companyDeckPdf.pages.map((page, index) => `<figure><figcaption>${String(index + 1).padStart(2, "0")} / 10</figcaption><img src="${page}" alt="리나랩 COMPANY 덱 ${index + 1}쪽"></figure>`).join("");
    return new Response(`<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${companyDeckPdf.title}</title><style>body{margin:0;background:#080808;color:#f6f6f6;font:16px/1.55 system-ui}main{max-width:1200px;margin:auto;padding:32px 24px 80px}a{color:inherit}h1{font-size:clamp(1.8rem,4vw,3rem)}p,figcaption{color:#b7b7b7}figure{margin:42px 0}img{display:block;width:100%;height:auto;border:1px solid #505050}</style><main><a href="/">← 갤러리</a><h1>${companyDeckPdf.title}</h1><p>원본 10쪽 덱의 구성을 바탕으로 현재 공식 프로젝트 내용으로 다시 제작했습니다.</p><p><a href="${companyDeckPdf.pdf}">PDF 다운로드 · 10쪽</a></p>${slides}</main></html>`, { headers: { "Content-Type": types[".html"], "X-Content-Type-Options": "nosniff" } });
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
