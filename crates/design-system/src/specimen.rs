//! The component specimen page: every component in both themes, plus one
//! right-to-left panel. It is the component documentation and the fixture the
//! visual regression job screenshots (`design/visual/regress.mjs`).

use crate::i18n::{Key, dir, t};
use crate::tokens::{Theme, css};

const COMPONENT_CSS: &str = r"
body { margin:0; font:var(--text-1)/1.5 'DejaVu Sans',sans-serif; }
section { background:var(--bg); color:var(--fg); padding:var(--space-5); width:720px; }
h1 { font-size:var(--text-4); margin:0 0 var(--space-4); }
.muted { color:var(--muted); }
.panel { background:var(--panel); border:1px solid var(--border); border-radius:8px;
         padding:var(--space-4); margin:var(--space-4) 0; }
.btn { background:var(--accent); color:var(--on-accent); border:0; border-radius:6px;
       min-height:var(--touch-target); padding:0 var(--space-5); font:inherit; font-weight:600; }
.btn.secondary { background:transparent; color:var(--accent); border:1px solid var(--accent); }
.focus { outline:2px solid var(--focus); outline-offset:2px; }
.warn { border-inline-start:4px solid var(--danger); color:var(--danger); }
.caution { border-inline-start:4px solid var(--warning); color:var(--warning); }
.ok { color:var(--success); }
table { width:100%; border-collapse:collapse; }
td,th { text-align:start; padding:var(--space-2); border-bottom:1px solid var(--border); }
th { color:var(--muted); font-weight:500; }
";

fn section(theme: Theme, locale: &str) -> String {
    let known = "0x1a2b…abcd";
    let d = dir(locale).attr();
    format!(
        r#"<section data-theme="{name}" dir="{d}" lang="{locale}" id="{name}-{locale}">
<h1>Maya2C · {name}</h1>
<p>Body text on the page background. <span class="muted">Muted secondary text.</span></p>
<p><button class="btn">{send}</button> <button class="btn secondary">{cancel}</button>
<button class="btn focus">{review}</button></p>
<div class="panel warn">{poison}</div>
<div class="panel caution">{lookalike}</div>
<div class="panel"><table>
<tr><th>Tx</th><th>Block</th><th>Status</th></tr>
<tr><td>7f3a09c1…e4d2</td><td>1204</td><td class="ok">confirmed</td></tr>
</table></div>
<p class="muted">{offline}</p>
</section>
"#,
        name = theme.name(),
        send = t(locale, Key::Send),
        cancel = t(locale, Key::Cancel),
        review = t(locale, Key::Review),
        poison = t(locale, Key::WarnPoisoning).replace("{known}", known),
        lookalike = t(locale, Key::WarnLookalike).replace("{known}", "USDC"),
        offline = t(locale, Key::OfflineBalance),
    )
}

/// The full specimen page. The screenshot job captures each `<section>`.
#[must_use]
pub fn page() -> String {
    let mut body = String::new();
    for theme in Theme::ALL {
        body.push_str(&section(theme, "en"));
    }
    body.push_str(&section(Theme::Calm, "ar"));
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n<title>Maya2C design system specimen</title>\n<style>\n{}{COMPONENT_CSS}</style></head>\n<body>\n{body}</body></html>\n",
        css()
    )
}
