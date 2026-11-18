//! The Maya2C Wallet driven end to end through its real UI: `WebView2` via
//! `tauri-driver` and `msedgedriver`, the release build of the application.
//!
//! Ignored by default because it needs a built application and two drivers.
//! Run it with:
//!
//! ```text
//! cargo tauri build --no-bundle          # in apps/wallet-gui/src-tauri
//! MAYA_WALLET_EXE=apps/wallet-gui/src-tauri/target/release/maya-wallet-gui.exe \
//! TAURI_DRIVER=.../tauri-driver.exe EDGE_DRIVER=.../msedgedriver.exe \
//! cargo test -p maya-wallet-e2e -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Optional: `AXE_CORE_JS` names `axe.min.js` (axe-core), and every checked
//! screen must then have zero WCAG 2.2 A/AA violations; `MAYA_E2E_SHOTS`
//! names a directory for screenshots in both themes.
//!
//! The application is started with its keychain moved to a namespace of this
//! run's own (`MAYA_WALLET_KEYCHAIN_SERVICE`), which is deleted afterwards:
//! the test never sees, overwrites or leaves behind the user's wallet entry.
//! The two tests share the driver's port, so they run one at a time.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use maya_wallet_e2e::stub_node::StubNode;
use maya_wallet_e2e::{Element, Session};

const DRIVER: &str = "http://127.0.0.1:4444";
const PASSPHRASE: &str = "correct horse battery staple e2e";
const WAIT: Duration = Duration::from_secs(60);
/// How long an optional element (one the old UI lacks) is waited for.
const BRIEF: Duration = Duration::from_secs(3);
/// Drawn on every picture taken against the stub node.
const STUB_CAPTION: &str =
    "E2E STUB NODE (e2e/src/stub_node.rs): height and balance are fixture data, not a live network";

fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must name the file; see this file's header"))
}

struct Driver(Child);

impl Drop for Driver {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Deletes this run's keychain entry however the test ends.
struct Keychain(String);

impl Drop for Keychain {
    fn drop(&mut self) {
        // keyring-rs on Windows stores `<name>.<service>` as a generic credential.
        let _ = Command::new("cmdkey")
            .arg(format!("/delete:primary.{}", self.0))
            .stdout(Stdio::null())
            .status();
    }
}

fn driver(service: &str) -> Driver {
    let child = Command::new(env("TAURI_DRIVER"))
        .args(["--native-driver", &env("EDGE_DRIVER")])
        .env("MAYA_WALLET_KEYCHAIN_SERVICE", service)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("starting tauri-driver");
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect("127.0.0.1:4444").is_err() {
        assert!(Instant::now() < deadline, "tauri-driver did not listen");
        std::thread::sleep(Duration::from_millis(200));
    }
    Driver(child)
}

/// A fresh application in a keychain namespace of its own.
fn launch(tag: &str) -> (Keychain, Driver, Session, String) {
    let service = format!("com.maya2c.wallet.e2e-{tag}-{}", std::process::id());
    let keychain = Keychain(service.clone());
    let driver = driver(&service);
    let app = std::path::absolute(env("MAYA_WALLET_EXE"))
        .unwrap()
        .display()
        .to_string();
    let s = Session::start(DRIVER, &app).unwrap();
    (keychain, driver, s, app)
}

fn button(s: &Session, label: &str) -> Element {
    s.xpath(&format!("//button[normalize-space()='{label}']"), WAIT)
        .unwrap()
}

/// A button the old UI may not have.
fn maybe_button(s: &Session, label: &str) -> Option<Element> {
    s.xpath(&format!("//button[normalize-space()='{label}']"), BRIEF)
        .ok()
}

/// A home action button: its text is an icon glyph and then `label`.
fn action(s: &Session, label: &str) -> Element {
    s.xpath(
        &format!("//button[contains(@class,'action') and contains(normalize-space(),'{label}')]"),
        WAIT,
    )
    .unwrap()
}

/// The input or textarea whose accessible name is `name`: found the way a
/// screen reader finds it, so a field without an associated label fails here.
fn field(s: &Session, name: &str) -> Element {
    let deadline = Instant::now() + WAIT;
    loop {
        for candidate in s.all("input, textarea").unwrap() {
            if s.label(&candidate).is_ok_and(|label| label.trim() == name) {
                return candidate;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no input has the accessible name {name:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The input right after the visible label `name`: the old Send form's shape
/// as well as the new one, so the lock test can run against both.
fn input_after_label(s: &Session, name: &str, wait: Duration) -> Option<Element> {
    s.xpath(
        &format!("//label[normalize-space()='{name}']/following-sibling::input[1]"),
        wait,
    )
    .ok()
}

/// The visible text of the `index`th match of `css`, once `ready` accepts
/// it (or [`WAIT`] passes). Screens fade in, and `WebDriver` reports an element
/// at opacity 0 as having no text, so a read waits for the text rather than
/// only for the element.
fn text_when(s: &Session, css: &str, index: usize, ready: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let rows = s.wait_for(css, index + 1, WAIT).unwrap();
        let text = s.text(&rows[index]).unwrap();
        if ready(&text) || Instant::now() > deadline {
            return text;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Forces a theme for a screenshot or a contrast check, as the Settings
/// control does: `data-theme` on the root element.
fn force_theme(s: &Session, theme: &str) {
    s.execute(&format!(
        "document.documentElement.dataset.theme = '{theme}'; return true"
    ))
    .unwrap();
}

/// The page widths (CSS px) every screen is checked at: phone first (360,
/// the narrowest window the app allows, `minWidth` in tauri.conf.json), the
/// common phones, a tablet, a laptop and a wide desktop.
const WIDTHS: [u32; 6] = [360, 390, 430, 768, 1024, 1440];
/// The widths every screen is pictured at: phone, tablet and desktop.
const SHOT_WIDTHS: [u32; 3] = [360, 768, 1440];
const DEFAULT_SIZE: (u32, u32) = (460, 780);
/// A phone's height, so what is "below the fold" in a picture is what is
/// below it on a phone.
const TALL: u32 = 740;

/// Sizes the window so the page itself is `width` CSS px wide: the window
/// frame and the scrollbar are measured and added, so a check "at 360" is
/// a check of a 360 px page.
fn at_width(s: &Session, width: u32) {
    let mut outer = width;
    for _ in 0..3 {
        s.resize(outer, TALL).unwrap();
        // Layout settles after the resize event; a read before it sees the old width.
        std::thread::sleep(Duration::from_millis(400));
        let inner = s
            .execute("return document.documentElement.clientWidth")
            .unwrap()
            .as_u64()
            .unwrap();
        let inner = u32::try_from(inner).unwrap();
        if inner == width {
            return;
        }
        outer = (outer + width).saturating_sub(inner);
    }
    let inner = s
        .execute("return document.documentElement.clientWidth")
        .unwrap();
    panic!("could not size the page to {width} px; it is {inner}");
}

fn restore_size(s: &Session) {
    s.resize(DEFAULT_SIZE.0, DEFAULT_SIZE.1).unwrap();
    // As in `at_width`: a click before the relayout lands where the element
    // was at the old size.
    std::thread::sleep(Duration::from_millis(400));
}

/// Saves screenshots at each of [`SHOT_WIDTHS`] in both themes when
/// `MAYA_E2E_SHOTS` names a directory, then puts the dark theme back.
fn shots(s: &Session, name: &str) {
    shots_captioned(s, name, None);
}

/// [`shots`], with `caption` drawn across the top of each picture: the
/// stub node's screens say so in the image itself, so no picture of a stub
/// balance can pass for a live one.
fn shots_captioned(s: &Session, name: &str, caption: Option<&str>) {
    let Some(dir) = std::env::var_os("MAYA_E2E_SHOTS") else {
        return;
    };
    if let Some(text) = caption {
        s.execute(&format!(
            "const c = document.createElement('div'); c.id = 'e2e-caption';\
             c.textContent = {text:?};\
             c.style.cssText = 'padding:5px 10px;font:600 12px/1.3 system-ui;text-align:center;\
             color:Canvas;background:CanvasText';\
             document.body.prepend(c); return true"
        ))
        .unwrap();
    }
    for width in SHOT_WIDTHS {
        at_width(s, width);
        for theme in ["dark", "light"] {
            force_theme(s, theme);
            // From the top, so the caption (and the header) are in the
            // picture whatever an earlier click scrolled to.
            s.execute("window.scrollTo(0, 0); return true").unwrap();
            // Past the screens' entrance, so the picture is what a person sees
            // once it settles, not a frame of the fade.
            std::thread::sleep(Duration::from_millis(700));
            let path = std::path::Path::new(&dir).join(format!("{name}-{width}-{theme}.png"));
            s.screenshot(&path).unwrap();
            println!("screenshot: {}", path.display());
        }
    }
    if caption.is_some() {
        s.execute("document.getElementById('e2e-caption').remove(); return true")
            .unwrap();
    }
    restore_size(s);
    force_theme(s, "dark");
}

/// Every visible element inside the window's width, at each of [`WIDTHS`]:
/// the "shrunken desktop" failure (a row or a hash pushing past the edge)
/// caught as a list of offenders, not as a picture somebody has to look at.
fn assert_no_overflow(s: &Session, screen: &str) {
    for width in WIDTHS {
        at_width(s, width);
        assert_no_overflow_now(s, &format!("{screen} @{width}"));
    }
    restore_size(s);
}

/// The header at phone width: the Lock control is a 44 px target on the same
/// centre line as the network pill.
fn assert_header_aligned(s: &Session) {
    at_width(s, WIDTHS[0]);
    let found = s
        .execute(
            "const lock = document.querySelector('.titlebar .lock').getBoundingClientRect();\
             const pill = document.querySelector('.titlebar .net-pill').getBoundingClientRect();\
             return [lock.width, lock.height, (lock.top + lock.bottom) / 2 - (pill.top + pill.bottom) / 2]",
        )
        .unwrap();
    let [w, h, offset] = [0, 1, 2].map(|i| found[i].as_f64().unwrap());
    println!("header @360: Lock {w}x{h}, centre offset from the pill {offset}");
    assert!(w >= 44.0 && h >= 44.0 && h <= 46.0, "Lock is {w}x{h}");
    assert!(
        offset.abs() <= 1.0,
        "Lock is {offset} px off the pill's centre line"
    );
    restore_size(s);
}

/// The primary button paints the accent gradient's own colours: the start
/// stop of its background is `--accent-grad-from`, not a paler mix.
fn assert_primary_is_the_accent(s: &Session) {
    let found = s
        .execute(
            "const b = document.querySelector('button.primary:not(:disabled)');\
             const probe = document.createElement('i');\
             probe.style.color = 'var(--accent-grad-from)'; document.body.append(probe);\
             const from = getComputedStyle(probe).color; probe.remove();\
             const cs = getComputedStyle(b);\
             return [cs.backgroundImage, from, cs.opacity, cs.filter]",
        )
        .unwrap();
    println!("primary: {found}");
    let image = found[0].as_str().unwrap();
    let from = found[1].as_str().unwrap();
    assert!(
        image.contains(from),
        "the primary's gradient does not start at {from}: {image}"
    );
    assert_eq!(found[2], "1", "the primary is not dimmed");
    // `none`, or the hover brightening (the pointer may rest on it): never
    // a filter that dulls it.
    let filter = found[3].as_str().unwrap();
    let dulled = filter
        .strip_prefix("brightness(")
        .and_then(|f| f.strip_suffix(')'))
        .is_some_and(|f| f.parse::<f64>().unwrap() < 1.0);
    assert!(
        filter == "none" || (filter.starts_with("brightness(") && !dulled),
        "{filter}"
    );
}

fn assert_no_overflow_now(s: &Session, screen: &str) {
    let found = s
        .execute(
            "const w = document.documentElement.clientWidth;\
             const out = [];\
             if (document.documentElement.scrollWidth > w) out.push('page scrollWidth ' + document.documentElement.scrollWidth + ' > ' + w);\
             const name = document.querySelector('.titlebar .name');\
             if (name && name.scrollWidth > name.clientWidth) out.push('the app name is cut off in the header');\
             for (const el of document.querySelectorAll('body *')) {\
               const r = el.getBoundingClientRect();\
               if (r.width === 0 || r.height === 0) continue;\
               if (r.right > w + 0.5 || r.left < -0.5) out.push(el.tagName.toLowerCase() + '.' + el.className + ' ' + Math.round(r.left) + '..' + Math.round(r.right) + ' of ' + w);\
             }\
             return out;",
        )
        .unwrap();
    let offenders: Vec<String> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "{screen}: content wider than the window: {offenders:#?}"
    );
}

/// Zero axe-core WCAG 2.2 A/AA violations in both themes, when `AXE_CORE_JS`
/// names axe-core's `axe.min.js`.
fn assert_accessible(s: &Session, screen: &str) {
    let Ok(path) = std::env::var("AXE_CORE_JS") else {
        println!("axe {screen}: skipped (AXE_CORE_JS unset)");
        return;
    };
    let axe = std::fs::read_to_string(&path).expect("reading AXE_CORE_JS");
    for theme in ["dark", "light"] {
        force_theme(s, theme);
        std::thread::sleep(Duration::from_millis(700));
        let script = format!(
            "const done = arguments[arguments.length - 1];\n{axe}\n;\
             window.axe.run(document, {{runOnly: {{type: 'tag', values: ['wcag2a','wcag2aa','wcag21a','wcag21aa','wcag22aa']}}}})\
             .then(r => done(r.violations.map(v => v.id + ' (' + v.nodes.length + '): ' + v.help + ' :: ' + v.nodes.map(n => n.target.join(' ')).join(' | '))))\
             .catch(e => done(['axe failed: ' + e]));"
        );
        let found = s.execute_async(&script).unwrap();
        let violations: Vec<String> = found
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        println!("axe {screen} {theme}: {} violation(s)", violations.len());
        assert!(violations.is_empty(), "{screen} ({theme}): {violations:#?}");
    }
    force_theme(s, "dark");
}

/// The recovery phrase the Backup screen shows, word by word.
fn shown_phrase(s: &Session) -> Vec<String> {
    let words = s.wait_for("ol.phrase li", 24, WAIT).unwrap();
    assert_eq!(words.len(), 24, "a 24-word recovery phrase");
    // Each item is <span class="index">n</span>word.
    s.execute(
        "return [...document.querySelectorAll('ol.phrase li')]\
         .map(li => li.lastChild.textContent.trim())",
    )
    .unwrap()
    .as_array()
    .unwrap()
    .iter()
    .map(|w| w.as_str().unwrap().to_string())
    .collect()
}

/// Answers the three-word check from `phrase`. Returns whether the screen
/// asked at all (the old UI did not).
fn answer_word_check(s: &Session, phrase: &[String], wait: Duration) -> bool {
    if s.wait_for(".quiz input", 3, wait).is_err() {
        return false;
    }
    for input in s.all(".quiz input").unwrap() {
        let name = s.label(&input).unwrap();
        let n: usize = name
            .trim()
            .strip_prefix("Word ")
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("a word-check input is named {name:?}, not \"Word n\""));
        s.replace_text(&input, &phrase[n - 1]).unwrap();
    }
    true
}

/// Creates a wallet on whichever setup flow the build has and returns its
/// phrase. Used by the lock test, which must also run against the old UI.
fn create_any(s: &Session) -> Vec<String> {
    if let Some(choice) = maybe_button(s, "Create a new wallet") {
        s.click(&choice).unwrap();
    }
    let fields = s.wait_for("input[type=password]", 2, WAIT).unwrap();
    s.type_text(&fields[0], PASSPHRASE).unwrap();
    s.type_text(&fields[1], PASSPHRASE).unwrap();
    let create = s
        .xpath(
            "//button[normalize-space()='Create wallet' or normalize-space()='Create new wallet']",
            WAIT,
        )
        .unwrap();
    s.click(&create).unwrap();
    let phrase = shown_phrase(s);
    s.click(&s.all("input[type=checkbox]").unwrap()[0]).unwrap();
    s.click(&button(s, "Continue")).unwrap();
    if answer_word_check(s, &phrase, BRIEF) {
        s.click(&button(s, "Open wallet")).unwrap();
    }
    text_when(s, ".accounts button", 0, |t| t.contains("Account 0"));
    phrase
}

/// Clicks Review once the node's fee terms have enabled it.
fn review(s: &Session) {
    let deadline = Instant::now() + WAIT;
    while !s.enabled(&button(s, "Review")).unwrap_or(false) {
        assert!(Instant::now() < deadline, "Review never became enabled");
        std::thread::sleep(Duration::from_millis(200));
    }
    s.click(&button(s, "Review")).unwrap();
}

/// Points the wallet at `url` through Settings.
fn use_node(s: &Session, url: &str) {
    let settings = s.wait_for("details.settings summary", 1, WAIT).unwrap();
    s.click(&settings[0]).unwrap();
    let endpoint = s.wait_for(".settings input.mono", 1, WAIT).unwrap();
    s.replace_text(&endpoint[0], url).unwrap();
    s.click(&button(s, "Refresh")).unwrap();
}

#[test]
#[ignore = "needs the built wallet, tauri-driver and msedgedriver; see the file header"]
fn create_back_up_add_an_account_lock_and_unlock() {
    let (_keychain, _driver, s, app) = launch("flow");

    // Welcome: the mark, two choices, and the testnet tag.
    let tag = text_when(&s, ".testnet-tag", 0, |t| !t.is_empty());
    assert_eq!(tag.trim(), "Testnet, coins have no value");
    assert_accessible(&s, "welcome");
    assert_no_overflow(&s, "welcome");
    shots(&s, "welcome");
    s.click(&button(&s, "Create a new wallet")).unwrap();

    // Setup: each passphrase field found by its accessible name, marked as a
    // new password for the OS's password manager, with show/hide and a hint.
    let pass = field(&s, "Passphrase");
    let confirm = field(&s, "Confirm passphrase");
    for input in [&pass, &confirm] {
        assert_eq!(
            s.attribute(input, "autocomplete").unwrap().as_deref(),
            Some("new-password")
        );
    }
    s.type_text(&pass, "short").unwrap();
    let hint = text_when(&s, ".strength", 0, |t| !t.is_empty());
    println!("strength for a 5-character passphrase: {hint}");
    s.replace_text(&pass, PASSPHRASE).unwrap();
    s.type_text(&confirm, PASSPHRASE).unwrap();
    // A toggle button: one constant computed name, the state in aria-pressed.
    let toggle = s.wait_for("button.reveal", 1, WAIT).unwrap();
    assert_eq!(s.label(&toggle[0]).unwrap(), "Show passphrase");
    s.click(&toggle[0]).unwrap();
    assert_eq!(
        s.attribute(&pass, "type").unwrap().as_deref(),
        Some("text"),
        "Show reveals the passphrase"
    );
    assert_eq!(
        s.attribute(&toggle[0], "aria-pressed").unwrap().as_deref(),
        Some("true")
    );
    assert_eq!(
        s.label(&toggle[0]).unwrap(),
        "Show passphrase",
        "the name does not flip while pressed"
    );
    s.click(&toggle[0]).unwrap();
    assert_eq!(
        s.attribute(&pass, "type").unwrap().as_deref(),
        Some("password")
    );
    assert_eq!(
        s.attribute(&toggle[0], "aria-pressed").unwrap().as_deref(),
        Some("false")
    );
    assert_primary_is_the_accent(&s);
    assert_accessible(&s, "setup");
    assert_no_overflow(&s, "setup");
    shots(&s, "setup");
    s.click(&button(&s, "Create wallet")).unwrap();

    // Backup: 24 words; Continue waits for the acknowledgement. If the app
    // refused instead, say what it said.
    s.wait_for("ol.phrase li, .banner.error", 1, WAIT)
        .unwrap_or_else(|e| {
            let tauri = s
                .execute(
                    "return typeof window.__TAURI__ + ' / ' + typeof window.__TAURI_INTERNALS__",
                )
                .ok();
            panic!("{e}; window.__TAURI__ / __TAURI_INTERNALS__: {tauri:?}")
        });
    if let Some(error) = s.all(".banner.error").unwrap().first() {
        panic!(
            "the app refused to create the wallet: {}",
            s.text(error).unwrap()
        );
    }
    let phrase = shown_phrase(&s);
    assert!(
        !s.all(".testnet-tag").unwrap().is_empty(),
        "the tag stays on backup"
    );
    let proceed = button(&s, "Continue");
    assert!(
        !s.enabled(&proceed).unwrap(),
        "Continue waits for the acknowledgement"
    );
    assert_accessible(&s, "backup");
    assert_no_overflow(&s, "backup");
    shots(&s, "backup");
    s.click(&s.all("input[type=checkbox]").unwrap()[0]).unwrap();
    assert!(s.enabled(&proceed).unwrap());
    s.click(&proceed).unwrap();

    // Three random words, each input named "Word n"; a wrong word keeps the
    // wallet closed.
    s.wait_for(".quiz input", 3, WAIT).unwrap();
    assert_accessible(&s, "backup check");
    assert_no_overflow(&s, "backup check");
    shots(&s, "backup-check");
    let open = button(&s, "Open wallet");
    let first = &s.all(".quiz input").unwrap()[0];
    s.type_text(first, "notaword").unwrap();
    assert!(
        !s.enabled(&open).unwrap(),
        "a wrong word keeps Open wallet disabled"
    );
    assert!(answer_word_check(&s, &phrase, WAIT));
    assert!(s.enabled(&open).unwrap());
    s.click(&open).unwrap();

    // Wallet: one account, then a second. The default node may be paused or
    // unreachable: that is a calm card, never a red banner.
    let first = text_when(&s, ".accounts button", 0, |t| t.contains("Account 0"));
    assert!(first.contains("Account 0"), "{first}");
    std::thread::sleep(Duration::from_secs(4));
    let errors = s.all(".banner.error").unwrap();
    assert!(
        errors.is_empty(),
        "an unreachable node raised a red banner: {:?}",
        errors.first().map(|e| s.text(e).unwrap())
    );
    println!(
        "default node: {}",
        if s.all(".state.paused").unwrap().is_empty() {
            "answered"
        } else {
            "paused card shown"
        }
    );
    // What a user sees today with the public testnet paused: never a figure
    // the node did not give.
    let balance = text_when(&s, ".hero-balance", 0, |t| !t.trim().is_empty());
    println!("default node balance line: {balance:?}");
    if !s.all(".state.paused").unwrap().is_empty() {
        assert_eq!(balance.trim(), "Balance unavailable");
    }
    assert_accessible(&s, "home (default node)");
    assert_no_overflow(&s, "home (default node)");
    assert_header_aligned(&s);
    shots(&s, "home-default-node");
    s.click(&action(&s, "Receive")).unwrap();
    let address = text_when(&s, ".copy-row code.address", 0, |t| t.trim().len() == 64);
    s.click(&button(&s, "Back")).unwrap();
    s.click(&button(&s, "Add account")).unwrap();
    let second = text_when(&s, ".accounts button", 1, |t| t.contains("Account 1"));
    assert!(second.contains("Account 1"), "{second}");

    // Receive: the address and a QR code the wallet core drew.
    s.click(&action(&s, "Receive")).unwrap();
    let qr = s.wait_for(".qr svg", 1, WAIT).unwrap();
    assert!(!qr.is_empty(), "the Receive screen draws a QR code");
    let shown = text_when(&s, ".copy-row code.address", 0, |t| t.trim().len() == 64);
    assert_eq!(shown.trim().len(), 64, "a full hex address: {shown}");
    s.click(&button(&s, "Back")).unwrap();
    s.wait_for(".accounts button", 2, WAIT).unwrap();

    // Lock; a wrong passphrase is refused with an alert; the right one opens.
    s.click(&button(&s, "Lock")).unwrap();
    let unlock = field(&s, "Passphrase");
    assert_eq!(
        s.attribute(&unlock, "autocomplete").unwrap().as_deref(),
        Some("current-password")
    );
    s.type_text(&unlock, "not the passphrase").unwrap();
    s.click(&button(&s, "Unlock")).unwrap();
    let banner = s.wait_for(".banner.error", 1, WAIT).unwrap();
    assert_eq!(
        s.attribute(&banner[0], "role").unwrap().as_deref(),
        Some("alert")
    );
    let dismiss = s.wait_for(".banner.error button.dismiss", 1, WAIT).unwrap();
    assert_eq!(s.label(&dismiss[0]).unwrap(), "Dismiss");
    println!("wrong passphrase: {}", s.text(&banner[0]).unwrap());
    let unlock = field(&s, "Passphrase");
    s.replace_text(&unlock, PASSPHRASE).unwrap();
    s.click(&button(&s, "Unlock")).unwrap();
    let rows = s.wait_for(".accounts button", 1, WAIT).unwrap();
    println!(
        "unlocked: {} account(s) derived; first {}",
        rows.len(),
        s.text(&rows[0]).unwrap().replace('\n', " ")
    );
    s.end().unwrap();

    // Relaunched, the app finds the stored wallet and asks to unlock.
    let s = Session::start(DRIVER, &app).unwrap();
    button(&s, "Unlock");
    assert_eq!(
        s.all("ol.phrase li").unwrap().len(),
        0,
        "the phrase is never shown again"
    );

    // Forgotten passphrase: recovery is reachable from Unlock, replaces the
    // stored wallet under a new passphrase, and derives the same address.
    s.click(&button(
        &s,
        "Forgot passphrase? Recover with your recovery phrase",
    ))
    .unwrap();
    let words = field(&s, "Recovery phrase");
    assert_eq!(
        s.attribute(&words, "autocomplete").unwrap().as_deref(),
        Some("off")
    );
    s.type_text(&field(&s, "Passphrase"), "a brand new passphrase")
        .unwrap();
    s.type_text(&field(&s, "Confirm passphrase"), "a brand new passphrase")
        .unwrap();
    s.type_text(&words, &phrase.join(" ")).unwrap();
    // Looked up again each time: the view may redraw the button while the
    // phrase is checked, leaving an earlier handle stale (seen 1 run in 5).
    let deadline = Instant::now() + WAIT;
    while !s.enabled(&button(&s, "Recover wallet")).unwrap_or(false) {
        assert!(
            Instant::now() < deadline,
            "the phrase never validated: hint {:?}, {} words typed",
            s.all("#setup-phrase-hint")
                .unwrap()
                .first()
                .map(|h| s.text(h).unwrap_or_default()),
            s.execute("return document.getElementById('setup-phrase').value.split(/\\s+/).filter(Boolean).length")
                .unwrap()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    s.click(&button(&s, "Recover wallet")).unwrap();
    text_when(&s, ".accounts button", 0, |t| t.contains("Account 0"));
    s.click(&action(&s, "Receive")).unwrap();
    let recovered = text_when(&s, ".copy-row code.address", 0, |t| t.trim().len() == 64);
    assert_eq!(
        recovered.trim(),
        address.trim(),
        "recovery derives the same account"
    );
    s.end().unwrap();
}

/// The Send review is a locked step. Signing replays the reviewed inputs, so
/// an edit after Review must take the review away: otherwise the screen shows
/// one transfer while the form holds another.
#[test]
#[ignore = "needs the built wallet, tauri-driver and msedgedriver; see the file header"]
fn editing_after_review_takes_the_review_away() {
    let node = StubNode::start().unwrap();
    let (_keychain, _driver, s, _app) = launch("send");
    create_any(&s);
    use_node(&s, node.url());
    // Wide enough that the pill shows the height (phone widths hide it).
    at_width(&s, 1024);
    let pill = text_when(&s, ".net-pill.test", 0, |t| t.contains("4,242"));
    assert!(pill.contains("4,242"), "the stub node answered: {pill}");
    restore_size(&s);

    // The lock first, so a build without it fails on the lock itself and
    // not on a layout or contrast check further down.
    s.click(&action(&s, "Receive")).unwrap();
    let own = text_when(&s, ".copy-row code.address", 0, |t| t.trim().len() == 64);
    s.click(&button(&s, "Back")).unwrap();
    s.click(&action(&s, "Send")).unwrap();
    let recipient = input_after_label(&s, "Recipient", WAIT).expect("a Recipient field");
    s.type_text(&recipient, own.trim()).unwrap();
    let amount = input_after_label(&s, "Amount", WAIT).expect("an Amount field");
    s.type_text(&amount, "1").unwrap();
    review(&s);
    let hero = text_when(&s, ".summary .amount-hero .value", 0, |t| t.trim() == "1");
    assert_eq!(
        hero.trim(),
        "1",
        "the summary leads with the reviewed amount"
    );
    let summary = text_when(&s, ".summary", 0, |t| !t.is_empty());
    println!("reviewed: {}", summary.replace('\n', " | "));

    // Locked: once reviewed, no field of the form is left to edit, and Edit
    // is the way back. Both unconditional: a build without the lock fails
    // here.
    let edit = maybe_button(&s, "Edit").expect("an Edit button on the reviewed transfer");
    let editable = s
        .execute(
            "return [...document.querySelectorAll('#send-recipient, #send-amount, #send-fee')]\
             .map(e => e.id)",
        )
        .unwrap();
    assert_eq!(
        editable,
        serde_json::json!([]),
        "the reviewed values are read-only text, not fields"
    );
    assert!(
        input_after_label(&s, "Amount", Duration::ZERO).is_none(),
        "no Amount input under review"
    );
    s.click(&edit).unwrap();
    let amount = input_after_label(&s, "Amount", WAIT).expect("an Amount field to edit");
    s.replace_text(&amount, "2").unwrap();
    std::thread::sleep(Duration::from_millis(700));

    let summaries = s.all(".summary").unwrap();
    assert!(
        summaries.is_empty(),
        "the review outlived an edit of the amount: {:?}",
        summaries.first().map(|e| s.text(e).unwrap_or_default())
    );
    let signers = s
        .all("button")
        .unwrap()
        .into_iter()
        .filter_map(|b| s.text(&b).ok())
        .filter(|t| t.starts_with("Sign") || (t.starts_with("Send ") && t.ends_with("MAYA")))
        .collect::<Vec<_>>();
    assert!(
        signers.is_empty(),
        "nothing can be signed before Review: {signers:?}"
    );

    // Review again: the summary is of the new amount.
    review(&s);
    let again = text_when(&s, ".summary .amount-hero .value", 0, |t| t.trim() == "2");
    assert_eq!(again.trim(), "2", "the new review is of the new amount");
    let send = s
        .xpath("//button[normalize-space()='Send 2 MAYA']", WAIT)
        .expect("the last button names the amount");
    assert!(s.enabled(&send).unwrap());
    let warning = text_when(&s, ".irreversible", 0, |t| !t.is_empty());
    assert_eq!(warning.trim(), "This cannot be undone.");
    // On a phone-sized window the Send button is on screen with the summary,
    // not below the fold.
    at_width(&s, WIDTHS[0]);
    let fold = s
        .execute(
            "const b = document.querySelector('.send-final').getBoundingClientRect();\
             return [b.top, b.bottom, window.innerHeight]",
        )
        .unwrap();
    println!("send button @360x{TALL}: {fold}");
    assert!(
        fold[1].as_f64().unwrap() <= fold[2].as_f64().unwrap(),
        "the Send button is below the fold at 360 px: {fold}"
    );
    restore_size(&s);
    assert_accessible(&s, "send review");
    assert_no_overflow(&s, "send review");
    shots_captioned(&s, "send-review-stub", Some(STUB_CAPTION));

    // Home with a node that answers.
    s.click(&button(&s, "Back")).unwrap();
    text_when(&s, ".accounts button", 0, |t| t.contains("Account 0"));
    assert_no_overflow(&s, "account");
    shots_captioned(&s, "account-stub", Some(STUB_CAPTION));
    // The Signed pill's bronze text on its tint, checked for contrast in
    // both themes. Signing itself waits on a native OK/Cancel dialog that
    // WebDriver cannot answer (and must not be bypassed), so the pill is a
    // sample row appended to the history card, styled by the app's own CSS.
    s.execute(
        "const tx = document.createElement('div'); tx.className = 'tx'; tx.id = 'e2e-pill-sample';\
         tx.innerHTML = '<div class=\"tx-head\"><span class=\"mono\">sample</span>\
         <span class=\"pill signed\">Signed</span></div>';\
         document.querySelector('.empty, .tx').after(tx); return true",
    )
    .unwrap();
    assert_accessible(&s, "account (Signed pill sample)");
    s.execute("document.getElementById('e2e-pill-sample').remove(); return true")
        .unwrap();

    // Theme: the Settings control sets the root's theme and the browser
    // chrome colour follows it.
    let settings = s.wait_for("details.settings summary", 1, WAIT).unwrap();
    s.click(&settings[0]).unwrap();
    s.click(&button(&s, "Light")).unwrap();
    let applied = s
        .execute(
            "const bg = getComputedStyle(document.documentElement).getPropertyValue('--bg').trim();\
             const metas = [...document.querySelectorAll('meta[name=theme-color]')].map(m => m.content);\
             return [document.documentElement.dataset.theme, bg, metas.every(c => c === bg)]",
        )
        .unwrap();
    assert_eq!(applied[0], "light", "{applied}");
    assert_eq!(
        applied[2], true,
        "theme-color follows the chosen theme: {applied}"
    );
    s.click(&button(&s, "Dark")).unwrap();
    s.end().unwrap();
}
