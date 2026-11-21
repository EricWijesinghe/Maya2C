// Use for every dynamic value written into innerHTML or a template string.
// Built pages and live feeds are both untrusted at the field level.
const HTML_ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

export function esc(value: unknown): string {
  return String(value).replace(/[&<>"']/g, (c) => HTML_ESCAPES[c] ?? c);
}

// Link targets: http(s) and same-site paths are kept; anything else becomes "#".
export function safeHref(url: unknown): string {
  const text = String(url);
  return /^(https?:\/\/|\/(?!\/))/i.test(text) ? esc(text) : "#";
}
