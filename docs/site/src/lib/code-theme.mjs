// Expressive Code (Shiki) themes built from design/tokens.json, so code blocks
// use the same roles as the rest of the site (ADR-136). Shiki needs literal
// colours, not custom properties, so the hex values are read here at build
// time; a token change reaches code blocks on the next build with no edit.
// Syntax colours are roles the token file already measures on surface-1
// (accent-text, bronze, steel, text-2: 4.68:1 or more in both themes).
import { readFileSync } from "node:fs";

const tokens = JSON.parse(readFileSync(new URL("../../../../design/tokens.json", import.meta.url), "utf8"));

/** `rgb(r g b / a)` -> `#rrggbbaa`; hex passes through. Shiki's colour parser does not read the space syntax. */
function hex(value) {
  const m = value.match(/^rgb\((\d+) (\d+) (\d+) \/ ([\d.]+)\)$/);
  if (!m) return value;
  const byte = (n) => Number(n).toString(16).padStart(2, "0");
  return `#${byte(m[1])}${byte(m[2])}${byte(m[3])}${byte(Math.round(Number(m[4]) * 255))}`;
}

function theme(name) {
  const t = Object.fromEntries(Object.entries(tokens.theme[name]).filter(([k]) => !k.startsWith("$")).map(([k, v]) => [k, hex(v)]));
  const scope = (scopes, foreground, fontStyle) => ({ scope: scopes, settings: { foreground, ...(fontStyle ? { fontStyle } : {}) } });
  return {
    name: `maya-${name}`,
    type: name,
    colors: {
      "editor.background": t["surface-1"],
      "editor.foreground": t.text,
      "editorLineNumber.foreground": t["text-3"],
      "editor.selectionBackground": t["accent-subtle"],
      focusBorder: t.accent,
      "editorGroupHeader.tabsBackground": t["surface-2"],
      "tab.activeBackground": t["surface-1"],
      "tab.activeForeground": t.text,
      "tab.inactiveForeground": t["text-2"],
      "tab.activeBorderTop": t.accent,
      "terminal.background": t["surface-1"],
    },
    tokenColors: [
      scope(["comment", "punctuation.definition.comment"], t["text-2"], "italic"),
      scope(["keyword", "storage", "storage.type", "storage.modifier", "keyword.control"], t["accent-text"]),
      scope(["string", "string.quoted", "markup.inline.raw"], t.bronze),
      scope(["constant", "constant.numeric", "constant.language", "entity.name.type", "support.type", "entity.name.class"], t.steel),
      scope(["entity.name.function", "support.function", "variable", "markup.heading"], t.text),
      scope(["punctuation", "keyword.operator", "meta.brace"], t["text-2"]),
      scope(["markup.inserted"], t.success),
      scope(["markup.deleted", "invalid"], t.danger),
    ],
  };
}

export const codeThemes = [theme("dark"), theme("light")];

/** Frame and chrome on the token scale: hairline border, no shadow, 8 px radius. */
export const codeStyleOverrides = {
  borderRadius: tokens.global.radius["r-md"],
  borderColor: "var(--border)",
  codeFontFamily: "var(--font-mono)",
  codeFontSize: tokens.global.type["type-mono"].size,
  codeLineHeight: tokens.global.type["type-mono"].line,
  uiFontFamily: "var(--font-sans)",
  uiFontSize: tokens.global.type["type-body-sm"].size,
  frames: {
    shadowColor: "transparent",
    frameBoxShadowCssValue: "none",
    editorActiveTabIndicatorTopColor: "var(--accent)",
    editorActiveTabIndicatorHeight: "1px",
    editorTabBarBorderBottomColor: "var(--border)",
    terminalTitlebarBorderBottomColor: "var(--border)",
    tooltipSuccessBackground: "var(--surface-3)",
    tooltipSuccessForeground: "var(--text)",
  },
};
