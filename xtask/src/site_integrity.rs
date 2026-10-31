//! `cargo xtask site-integrity` — checks that maya2c.dev serves what the site
//! build produced.
//!
//! Incident 2026-10-06: a stolen Cloudflare token injected a script into every
//! page. The only watch was a byte count run from one PC, which could not say
//! whether a 5558-byte difference was an attack or CDN lag, because it kept no
//! copy of the body. This module makes the comparison provable:
//!
//! - `sri <dist>` adds `integrity="sha384-…"` to every same-origin
//!   `<script src>` and `<link rel=stylesheet>` in the built HTML. A browser
//!   then refuses a script the edge altered, whether or not anyone checks.
//! - `manifest <dist> <out> [--commit SHA]` records a SHA-256 for every served
//!   HTML, JS and CSS file. HTML is hashed after `normalize`.
//! - `check <manifest> <live_dir>` hashes the bodies fetched from the live
//!   site the same way and fails on any difference, printing each one.
//!
//! `normalize` removes or decodes only what Cloudflare's edge adds by itself:
//! the Web Analytics beacon, the email-decode script and email-protection
//! links. Each pattern is exact, because a loose one (any `/cdn-cgi/` script)
//! is a place an injected script could hide.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use regex::{Captures, Regex};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha384};

const MANIFEST_NAME: &str = "integrity-manifest.json";
const SCHEMA: u64 = 1;
const BEACON_SRC: &str = "https://static.cloudflareinsights.com/beacon.min.js";

/// Cloudflare's `email-decode.min.js` lives under a per-zone hash directory.
const EMAIL_DECODE_SRC: &str =
    r"^/cdn-cgi/scripts/[0-9a-f]+/cloudflare-static/email-decode\.min\.js$";

static SCRIPT_TAG: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?is)<script\b([^>]*)>\s*</script>\s*").expect("static pattern compiles")
});
static CF_MAIL_HREF: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"<a href="/cdn-cgi/l/email-protection#([0-9a-fA-F]+)""#)
        .expect("static pattern compiles")
});
static CF_MAIL_TEXT: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"<span class="__cf_email__" data-cfemail="([0-9a-fA-F]+)">[^<]*</span>"#)
        .expect("static pattern compiles")
});
static EMAIL_DECODE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(EMAIL_DECODE_SRC).expect("static pattern compiles"));
static TAG: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?is)<(script|link)\b[^>]*>").expect("static pattern compiles")
});

pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, dist] if cmd == "sri" => sri_dir(Path::new(dist)),
        [cmd, dist, out, rest @ ..] if cmd == "manifest" => {
            let commit = match rest {
                [flag, sha] if flag == "--commit" => sha.as_str(),
                [] => "unknown",
                _ => return Err("usage: manifest <dist> <out> [--commit SHA]".into()),
            };
            let manifest = build_manifest(Path::new(dist), commit)?;
            fs::write(out, serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?)
                .map_err(|e| format!("write {out}: {e}"))
        }
        [cmd, manifest, live] if cmd == "check" => {
            let mismatches = check(Path::new(manifest), Path::new(live))?;
            if mismatches == 0 {
                Ok(())
            } else {
                Err(format!("{mismatches} served file(s) differ from the deployed build"))
            }
        }
        _ => Err("usage: site-integrity sri <dist> | manifest <dist> <out> [--commit SHA] | check <manifest> <live_dir>".into()),
    }
}

/// Decodes a Cloudflare email-protection hex string: the first byte is the XOR
/// key for the rest. `None` when the hex is malformed, so the caller leaves the
/// text alone and the page then fails the comparison instead of passing.
fn cf_decode(hex: &str) -> Option<String> {
    let bytes = hex::decode(hex).ok()?;
    let (&key, rest) = bytes.split_first()?;
    if rest.is_empty() {
        return None;
    }
    String::from_utf8(rest.iter().map(|b| b ^ key).collect()).ok()
}

fn is_cloudflare_script(attrs: &str) -> bool {
    let Some(src) = attr(attrs, "src") else {
        return false;
    };
    let beacon = src.starts_with(BEACON_SRC) && attrs.contains("data-cf-beacon=");
    beacon || EMAIL_DECODE.is_match(&src)
}

/// Undoes Cloudflare's own rewrites so built and served HTML compare equal.
/// Anything else (an injected script, a changed link) is left in place.
pub fn normalize(html: &str) -> String {
    let no_scripts = SCRIPT_TAG.replace_all(html, |c: &Captures| {
        if is_cloudflare_script(&c[1]) {
            String::new()
        } else {
            c[0].to_string()
        }
    });
    let mail_href = CF_MAIL_HREF.replace_all(&no_scripts, |c: &Captures| match cf_decode(&c[1]) {
        Some(email) => format!(r#"<a href="mailto:{email}""#),
        None => c[0].to_string(),
    });
    CF_MAIL_TEXT
        .replace_all(&mail_href, |c: &Captures| {
            cf_decode(&c[1]).unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?is)(?:^|\s){name}\s*=\s*"([^"]*)""#)).ok()?;
    re.captures(tag).map(|c| c[1].to_string())
}

/// Hash for one file. HTML goes through `normalize`; scripts and styles are
/// exact bytes, because Cloudflare does not rewrite them.
fn digest(rel: &str, bytes: &[u8]) -> String {
    let data = if extension(rel) == Some("html") {
        normalize(&String::from_utf8_lossy(bytes)).into_bytes()
    } else {
        bytes.to_vec()
    };
    hex::encode(Sha256::digest(&data))
}

fn extension(rel: &str) -> Option<&str> {
    Path::new(rel).extension().and_then(|e| e.to_str())
}

fn is_served(rel: &str) -> bool {
    matches!(extension(rel), Some("html" | "js" | "mjs" | "css"))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if is_served(&rel) && rel != MANIFEST_NAME {
            out.push(rel);
        }
    }
    Ok(())
}

fn build_manifest(dist: &Path, commit: &str) -> Result<Value, String> {
    let mut rels = Vec::new();
    collect(dist, dist, &mut rels)?;
    rels.sort();
    let mut files = serde_json::Map::new();
    for rel in &rels {
        let bytes = fs::read(dist.join(rel)).map_err(|e| format!("read {rel}: {e}"))?;
        files.insert(rel.clone(), json!(digest(rel, &bytes)));
    }
    if files.is_empty() {
        return Err(format!(
            "no HTML/JS/CSS under {}; did the build run?",
            dist.display()
        ));
    }
    Ok(json!({ "schema": SCHEMA, "commit": commit, "files": files }))
}

/// Returns the number of differing files. A missing live file counts.
fn check(manifest_path: &Path, live: &Path) -> Result<usize, String> {
    let text = fs::read_to_string(manifest_path)
        .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;
    let manifest: Value = serde_json::from_str(&text).map_err(|e| format!("manifest: {e}"))?;
    if manifest["schema"].as_u64() != Some(SCHEMA) {
        return Err(format!("manifest schema is not {SCHEMA}"));
    }
    let files = manifest["files"]
        .as_object()
        .ok_or("manifest has no `files` object")?;
    let mut mismatches = 0;
    for (rel, expected) in files {
        let expected = expected.as_str().unwrap_or_default();
        let path: PathBuf = live.join(rel);
        match fs::read(&path) {
            Err(_) => {
                println!("MISSING  {rel}");
                mismatches += 1;
            }
            Ok(bytes) => {
                let got = digest(rel, &bytes);
                if got != expected {
                    println!("MISMATCH {rel} expected={expected} got={got}");
                    mismatches += 1;
                }
            }
        }
    }
    println!(
        "site-integrity: commit {} checked {} files, {mismatches} mismatch(es)",
        manifest["commit"].as_str().unwrap_or("unknown"),
        files.len()
    );
    Ok(mismatches)
}

/// Adds SRI to same-origin scripts and stylesheets. Tags that already carry
/// `integrity=` are left alone. A referenced file that is missing fails the
/// build, because a page that points at nothing is a broken deploy.
fn sri_html(root: &Path, html: &str) -> Result<String, String> {
    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    for m in TAG.find_iter(html) {
        let tag = m.as_str();
        out.push_str(&html[last..m.start()]);
        last = m.end();
        let is_script = tag.to_ascii_lowercase().starts_with("<script");
        let url = if is_script {
            attr(tag, "src")
        } else if attr(tag, "rel").is_some_and(|r| r.eq_ignore_ascii_case("stylesheet")) {
            attr(tag, "href")
        } else {
            None
        };
        let same_origin = url
            .as_deref()
            .is_some_and(|u| u.starts_with('/') && !u.starts_with("//"));
        if tag.contains("integrity=") || !same_origin {
            out.push_str(tag);
            continue;
        }
        let url = url.unwrap_or_default();
        let path = url.split(['?', '#']).next().unwrap_or_default();
        let file = root.join(path.trim_start_matches('/'));
        let bytes =
            fs::read(&file).map_err(|e| format!("SRI: {url} is referenced but unreadable: {e}"))?;
        let sri = format!("sha384-{}", STANDARD.encode(Sha384::digest(&bytes)));
        let end = if tag.ends_with("/>") {
            tag.len() - 2
        } else {
            tag.len() - 1
        };
        out.push_str(&tag[..end]);
        out.push_str(" integrity=\"");
        out.push_str(&sri);
        out.push('"');
        out.push_str(&tag[end..]);
    }
    out.push_str(&html[last..]);
    Ok(out)
}

fn sri_dir(dist: &Path) -> Result<(), String> {
    let mut rels = Vec::new();
    collect(dist, dist, &mut rels)?;
    let mut tagged = 0;
    for rel in rels.iter().filter(|r| extension(r) == Some("html")) {
        let file = dist.join(rel);
        let html = fs::read_to_string(&file).map_err(|e| format!("read {rel}: {e}"))?;
        let next = sri_html(dist, &html)?;
        if next != html {
            tagged += 1;
            fs::write(&file, next).map_err(|e| format!("write {rel}: {e}"))?;
        }
    }
    println!(
        "site-integrity: SRI added to {tagged} HTML page(s) of {}",
        rels.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("site-integrity-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Encodes like Cloudflare: a key byte, then each character XOR the key.
    fn cf_encode(key: u8, s: &str) -> String {
        let mut out = format!("{key:02x}");
        for b in s.bytes() {
            out.push_str(&format!("{:02x}", b ^ key));
        }
        out
    }

    const PAGE: &str = r#"<html><head><script type="module" src="/_astro/a.js"></script></head><body><p>Write to <a href="mailto:ops@maya2c.dev">ops@maya2c.dev</a>.</p><script>console.log(1)</script></body></html>"#;

    #[test]
    fn cloudflare_email_and_beacon_normalize_to_built_page() {
        let email = cf_encode(0x42, "ops@maya2c.dev");
        let served = format!(
            r#"<html><head><script type="module" src="/_astro/a.js"></script></head><body><p>Write to <a href="/cdn-cgi/l/email-protection#{email}" class="x"><span class="__cf_email__" data-cfemail="{email}">[email&#160;protected]</span></a>.</p><script>console.log(1)</script><script defer src="https://static.cloudflareinsights.com/beacon.min.js/vcd15cbe" data-cf-beacon='{{"token":"t"}}'></script></body></html>"#
        );
        let built = r#"<html><head><script type="module" src="/_astro/a.js"></script></head><body><p>Write to <a href="mailto:ops@maya2c.dev" class="x">ops@maya2c.dev</a>.</p><script>console.log(1)</script></body></html>"#;
        assert_eq!(normalize(&served), normalize(built));
    }

    #[test]
    fn injected_script_is_not_normalized_away() {
        let dist = scratch("inject-dist");
        fs::write(dist.join("index.html"), PAGE).unwrap();
        let manifest = build_manifest(&dist, "abc").unwrap();
        let mpath = dist.join("manifest.json");
        fs::write(&mpath, manifest.to_string()).unwrap();
        let live = scratch("inject-live");
        let injected = PAGE.replace(
            "</body>",
            r#"<script src="https://evil.example/x.js"></script></body>"#,
        );
        fs::write(live.join("index.html"), injected).unwrap();
        assert_eq!(check(&mpath, &live).unwrap(), 1);
    }

    #[test]
    fn email_obfuscated_live_page_passes_check() {
        let dist = scratch("pass-dist");
        fs::write(dist.join("index.html"), PAGE).unwrap();
        fs::write(dist.join("app.js"), "export const x = 1;").unwrap();
        let mpath = dist.join("manifest.json");
        fs::write(&mpath, build_manifest(&dist, "abc").unwrap().to_string()).unwrap();
        let live = scratch("pass-live");
        let email = cf_encode(0x42, "ops@maya2c.dev");
        let cf_anchor = format!(
            r#"<a href="/cdn-cgi/l/email-protection#{email}"><span class="__cf_email__" data-cfemail="{email}">[email&#160;protected]</span></a>"#
        );
        let served = PAGE
            .replace(r#"<a href="mailto:ops@maya2c.dev">ops@maya2c.dev</a>"#, &cf_anchor)
            .replace("</body>", r#"<script defer src="https://static.cloudflareinsights.com/beacon.min.js/v1" data-cf-beacon='{}'></script></body>"#);
        fs::write(live.join("index.html"), served).unwrap();
        fs::write(live.join("app.js"), "export const x = 1;").unwrap();
        assert_eq!(check(&mpath, &live).unwrap(), 0);
    }

    #[test]
    fn changed_js_and_missing_file_are_counted() {
        let dist = scratch("js-dist");
        fs::write(dist.join("index.html"), PAGE).unwrap();
        fs::write(dist.join("app.js"), "export const x = 1;").unwrap();
        let mpath = dist.join("manifest.json");
        fs::write(&mpath, build_manifest(&dist, "abc").unwrap().to_string()).unwrap();
        let live = scratch("js-live");
        fs::write(live.join("index.html"), PAGE).unwrap();
        fs::write(live.join("app.js"), "export const x = 2;").unwrap();
        assert_eq!(check(&mpath, &live).unwrap(), 1, "changed js");
        fs::remove_file(live.join("app.js")).unwrap();
        assert_eq!(
            check(&mpath, &live).unwrap(),
            1,
            "missing js counts as a mismatch"
        );
    }

    #[test]
    fn malformed_cloudflare_hex_is_not_decoded() {
        assert_eq!(cf_decode("zz"), None);
        assert_eq!(cf_decode("4"), None);
        assert_eq!(cf_decode("42"), None, "key with no payload");
    }

    #[test]
    fn sri_adds_hash_once_and_skips_external_and_inline() {
        let dist = scratch("sri");
        fs::create_dir_all(dist.join("_astro")).unwrap();
        fs::write(dist.join("_astro/a.js"), "export {};").unwrap();
        let html = r#"<head><script type="module" src="/_astro/a.js"></script><script src="https://cdn.example/x.js"></script><script>1</script><link rel="stylesheet" href="/_astro/a.js"></head>"#;
        let once = sri_html(&dist, html).unwrap();
        let expect = format!("sha384-{}", STANDARD.encode(Sha384::digest(b"export {};")));
        assert_eq!(once.matches(&expect).count(), 2);
        assert!(once.contains(r#"<script src="https://cdn.example/x.js"></script>"#));
        assert_eq!(sri_html(&dist, &once).unwrap(), once, "idempotent");
    }

    #[test]
    fn sri_fails_on_missing_referenced_file() {
        let dist = scratch("sri-missing");
        let html = r#"<script src="/_astro/gone.js"></script>"#;
        assert!(sri_html(&dist, html).is_err());
    }

    #[test]
    fn base64_standard_vectors() {
        assert_eq!(STANDARD.encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(STANDARD.encode(b"fo"), "Zm8=");
    }
}
