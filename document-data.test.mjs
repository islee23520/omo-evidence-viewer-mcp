import { expect, test } from "bun:test";
import * as XLSX from "xlsx";
import { prepareDocuments } from "./document-data.mjs";

test("markdown renders formatting while escaping raw HTML and rejecting dangerous URLs", async () => {
  const source = '# Title\n\n**bold** & text\n\n<script>alert(1)</script><img src=x onerror=alert(1)>\n\n[safe](https://example.com) [bad](javascript:alert%281%29) [mixed](JaVaScRiPt:alert%281%29) [entity](java&#x73;cript:alert%281%29) [data](data:text/html,evil) ![image](javascript:evil)';
  const file = new File([source], "notes.MARKDOWN");
  const [document] = await prepareDocuments([file]);
  expect(document.kind).toBe("markdown");
  expect(document.html).toContain("<h1>Title</h1>");
  expect(document.html).toContain("<strong>bold</strong>");
  expect(document.html).toContain("&lt;script&gt;");
  const links = [];
  const images = [];
  await new HTMLRewriter().on("a", { element: element => links.push(element.getAttribute("href")) })
    .on("img", { element: element => images.push(element.getAttribute("src")) })
    .on("script", { element() { throw new Error("Raw script survived"); } })
    .transform(new Response(document.html)).text();
  expect(links[0]).toBe("https://example.com");
  expect(links.slice(1).every(value => value === null || !/^javascript:|^data:/i.test(value))).toBe(true);
  expect(images).toEqual([null]);
  expect(await file.text()).toBe(source);
});

test("CSV preserves commas, multiline CRLF, escaped quotes, blanks and hostile text", async () => {
  const source = '\uFEFFname,value,extra\r\n"Doe, Jane","line 1\r\nline 2","said ""hello"""\r\n"<script>alert(1)</script>",=SUM(A1),\r\n,,\r\n';
  const file = new File([source], "values.csv");
  const [document] = await prepareDocuments([file]);
  expect(document).toEqual({ name: "values.csv", href: "values.csv", kind: "csv", sheets: [{ name: "values.csv", rows: [
    ["name", "value", "extra"], ["Doe, Jane", "line 1\r\nline 2", 'said "hello"'],
    ["<script>alert(1)</script>", "=SUM(A1)", ""], ["", "", ""],
  ] }] });
  expect(new Uint8Array(await file.arrayBuffer())).toEqual(new TextEncoder().encode(source));
});

test("CSV handles empty files, terminal empty fields and rejects malformed quotes", async () => {
  for (const [source, rows] of [["", []], ['""', [[""]]], ["a,", [["a", ""]]], ["a\nb", [["a"], ["b"]]], ["\n", [[""]]]]) {
    expect((await prepareDocuments([new File([source], "test.csv")]))[0].sheets[0].rows).toEqual(rows);
  }
  for (const source of ['"unclosed', 'a"b', '"closed"junk', '"a""']) {
    await expect(prepareDocuments([new File([source], "invalid.csv")])).rejects.toThrow(SyntaxError);
  }
});

test("XLSX parses actual sheets and preserves formatted displayed values and empty cells", async () => {
  const workbook = XLSX.utils.book_new();
  const first = XLSX.utils.aoa_to_sheet([["Label", "Value", "Blank"], ["Price", 1234.5, ""], ["Rate", 0.125], ["Date", 45292], ["Boolean", true], ["Formula", 3]]);
  first.B2.z = '"$"#,##0.00';
  first.B3.z = "0.0%";
  first.B4.z = "yyyy-mm-dd";
  first.B6.f = "1+2";
  XLSX.utils.book_append_sheet(workbook, first, "Displayed");
  XLSX.utils.book_append_sheet(workbook, XLSX.utils.aoa_to_sheet([["Unicode", "한국어"], [], ["Quoted", 'a,"b"']]), "Second");
  XLSX.utils.book_append_sheet(workbook, XLSX.utils.aoa_to_sheet([]), "Empty");
  const bytes = XLSX.write(workbook, { type: "array", bookType: "xlsx", compression: true });
  const file = new File([bytes], "workbook.xlsx");
  const [document] = await prepareDocuments([file]);
  expect(document.kind).toBe("xlsx");
  expect(document.sheets).toEqual([
    { name: "Displayed", rows: [["Label", "Value", "Blank"], ["Price", "$1,234.50", ""], ["Rate", "12.5%", ""], ["Date", "2024-01-01", ""], ["Boolean", "TRUE", ""], ["Formula", "3", ""]] },
    { name: "Second", rows: [["Unicode", "한국어"], ["", ""], ["Quoted", 'a,"b"']] },
    { name: "Empty", rows: [] },
  ]);
  expect(new Uint8Array(await file.arrayBuffer())).toEqual(new Uint8Array(bytes));
});

test("PDF hrefs are relative and encoded and supported input order is retained", async () => {
  const files = [new File(["pdf"], "report #1?.PDF"), new File(["ignored"], "image.png"), new File(["# End"], "end.md")];
  const documents = await prepareDocuments(files);
  expect(documents[0]).toEqual({ name: "report #1?.PDF", href: "report%20%231%3F.PDF", kind: "pdf" });
  expect(documents.map(document => document.name)).toEqual(["report #1?.PDF", "end.md"]);
  expect(await prepareDocuments([])).toEqual([]);
});

test("documents are not truncated", async () => {
  const rows = Array.from({ length: 12000 }, (_, index) => `${index},value ${index}`).join("\n");
  const markdown = "text\n\n".repeat(12000) + "**final marker**";
  const [csv, md] = await prepareDocuments([new File([rows], "large.csv"), new File([markdown], "large.md")]);
  expect(csv.sheets[0].rows).toHaveLength(12000);
  expect(csv.sheets[0].rows.at(-1)).toEqual(["11999", "value 11999"]);
  expect(md.html).toContain("<strong>final marker</strong>");
});
