// Gives every body cell of a Markdown table the text of its column header as
// `data-label`, so a phone can show each row as a stacked card of label/value
// pairs (maya-living.css, max-width 600px) instead of a table cut off at the
// right edge (owner, 2026-11-14: "some objects go over the display").
// A headerless table (`| | |`) gets empty labels; the first cell of each row
// is then the row's title, which is how those tables are written.

const text = (node) =>
  node.type === "text" ? node.value : (node.children ?? []).map(text).join("");

const cells = (row) => (row.children ?? []).filter((c) => c.type === "element" && (c.tagName === "td" || c.tagName === "th"));

function label(table) {
  const sections = (table.children ?? []).filter((c) => c.type === "element");
  const head = sections.find((s) => s.tagName === "thead");
  const headRow = head?.children?.find((r) => r.type === "element" && r.tagName === "tr");
  const names = headRow ? cells(headRow).map((c) => text(c).trim()) : [];
  for (const body of sections.filter((s) => s.tagName === "tbody")) {
    for (const row of body.children ?? []) {
      if (row.type !== "element" || row.tagName !== "tr") continue;
      cells(row).forEach((cell, i) => {
        cell.properties = { ...cell.properties, dataLabel: names[i] ?? "" };
      });
    }
  }
}

function walk(node) {
  if (node.type === "element" && node.tagName === "table") label(node);
  for (const child of node.children ?? []) walk(child);
}

export default function rehypeTableLabels() {
  return (tree) => walk(tree);
}
