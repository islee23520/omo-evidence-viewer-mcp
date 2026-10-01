import { extname } from "node:path";

async function markdownHtml(source) {
  const escaped = source.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
  const html = Bun.markdown.html(escaped);
  return new HTMLRewriter().on("a[href], img[src]", {
    element(element) {
      const attribute = element.tagName === "a" ? "href" : "src";
      const value = element.getAttribute(attribute);
      const protocol = new URL(value, "https://documents.invalid/").protocol;
      const allowed = attribute === "href" ? ["http:", "https:", "mailto:"] : ["http:", "https:"];
      if (!allowed.includes(protocol)) element.removeAttribute(attribute);
    },
  }).transform(new Response(html)).text();
}

function csvRows(source) {
  const text = source.replace(/^\uFEFF/, "");
  if (!text) return [];
  const rows = [];
  let row = [];
  let field = "";
  let quoted = false;
  let closed = false;
  for (let i = 0; i < text.length; i++) {
    const character = text[i];
    if (quoted) {
      if (character === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; }
        else { quoted = false; closed = true; }
      } else field += character;
    } else if (character === "," || character === "\n" || character === "\r") {
      row.push(field);
      field = "";
      closed = false;
      if (character !== ",") {
        rows.push(row);
        row = [];
        if (character === "\r" && text[i + 1] === "\n") i++;
      }
    } else if (character === '"' && field === "" && !closed) {
      quoted = true;
    } else {
      if (closed || character === '"') throw new SyntaxError("Invalid CSV quoting");
      field += character;
    }
  }
  if (quoted) throw new SyntaxError("Unterminated CSV quoted field");
  if (row.length || field || closed || text.endsWith(",")) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

// Files remain untouched; href points at the original sibling asset.
export async function prepareDocuments(files) {
  const documents = [];
  for (const file of files) {
    const extension = extname(file.name).toLowerCase();
    const document = { name: file.name, href: file.name.split("/").map(encodeURIComponent).join("/") };
    if (extension === ".md" || extension === ".markdown") {
      documents.push({ ...document, kind: "markdown", html: await markdownHtml(await file.text()) });
    } else if (extension === ".csv") {
      documents.push({ ...document, kind: "csv", sheets: [{ name: file.name, rows: csvRows(await file.text()) }] });
    } else if (extension === ".xlsx") {
      // SheetJS is required for ZIP relationships, shared strings, and Excel number formats.
      const XLSX = await import("xlsx");
      const workbook = XLSX.read(await file.arrayBuffer(), { type: "array", cellText: true });
      const sheets = workbook.SheetNames.map(name => ({
        name,
        rows: XLSX.utils.sheet_to_json(workbook.Sheets[name], { header: 1, raw: false, defval: "", blankrows: true }),
      }));
      documents.push({ ...document, kind: "xlsx", sheets });
    } else if (extension === ".pdf") {
      documents.push({ ...document, kind: "pdf" });
    }
  }
  return documents;
}
