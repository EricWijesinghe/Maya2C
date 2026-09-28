//! The Maya Wallet driven end to end through its real UI: `WebView2` via
//! `tauri-driver` and `msedgedriver`, the release build of the application.
//!
//! Ignored by default because it needs a built application and two drivers.
//! Run it with:
//!
//! ```text
//! cargo tauri build --no-bundle          # in apps/wallet-gui/src-tauri
//! MAYA_WALLET_EXE=apps/wallet-gui/src-tauri/target/release/maya-wallet.exe \
//! TAURI_DRIVER=.../tauri-driver.exe EDGE_DRIVER=.../msedgedriver.exe \
//! cargo test -p maya-wallet-e2e -- --ignored --nocapture
//! ```
//!
//! The application is started with its keychain moved to a namespace of this
//! run's own (`MAYA_WALLET_KEYCHAIN_SERVICE`), which is deleted afterwards:
//! the test never sees, overwrites or leaves behind the user's wallet entry.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use maya_wallet_e2e::Session;

const DRIVER: &str = "http://127.0.0.1:4444";
const PASSPHRASE: &str = "correct horse battery staple e2e";
const WAIT: Duration = Duration::from_secs(60);

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
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect("127.0.0.1:4444").is_err() {
        assert!(
            std::time::Instant::now() < deadline,
            "tauri-driver did not listen"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    Driver(child)
}

fn button(s: &Session, label: &str) -> maya_wallet_e2e::Element {
    s.xpath(&format!("//button[normalize-space()='{label}']"), WAIT)
        .unwrap()
}

#[test]
#[ignore = "needs the built wallet, tauri-driver and msedgedriver; see the file header"]
fn create_back_up_add_an_account_lock_and_unlock() {
    let service = format!("com.maya2c.wallet.e2e-{}", std::process::id());
    let _keychain = Keychain(service.clone());
    let _driver = driver(&service);
    let app = std::path::absolute(env("MAYA_WALLET_EXE"))
        .unwrap()
        .display()
        .to_string();
    let s = Session::start(DRIVER, &app).unwrap();

    // Setup: a fresh namespace has no wallet, so the app opens at setup.
    let fields = s.wait_for("input[type=password]", 2, WAIT).unwrap();
    s.type_text(&fields[0], PASSPHRASE).unwrap();
    s.type_text(&fields[1], PASSPHRASE).unwrap();
    s.click(&button(&s, "Create new wallet")).unwrap();

    // Backup: 24 words, and Continue stays disabled until acknowledged. If
    // the app refused instead, say what it said.
    let appeared = s
        .wait_for("ol.phrase li, .banner.error", 1, WAIT)
        .unwrap_or_else(|e| {
            let tauri = s
                .execute(
                    "return typeof window.__TAURI__ + ' / ' + typeof window.__TAURI_INTERNALS__",
                )
                .ok();
            panic!("{e}; window.__TAURI__ / __TAURI_INTERNALS__: {tauri:?}")
        });
    let banner = s.all(".banner.error").unwrap();
    if let Some(error) = banner.first() {
        panic!(
            "the app refused to create the wallet: {}",
            s.text(error).unwrap()
        );
    }
    drop(appeared);
    let words = s.wait_for("ol.phrase li", 24, WAIT).unwrap();
    assert_eq!(words.len(), 24, "a 24-word recovery phrase");
    let proceed = button(&s, "Continue");
    assert!(
        !s.enabled(&proceed).unwrap(),
        "Continue waits for the acknowledgement"
    );
    s.click(&s.all("input[type=checkbox]").unwrap()[0]).unwrap();
    assert!(s.enabled(&proceed).unwrap());
    s.click(&proceed).unwrap();

    // Wallet: one account, then a second.
    let rows = s.wait_for(".accounts button", 1, WAIT).unwrap();
    let first = s.text(&rows[0]).unwrap();
    assert!(first.contains("Account 0"), "{first}");
    s.click(&button(&s, "Add account")).unwrap();
    let rows = s.wait_for(".accounts button", 2, WAIT).unwrap();
    assert!(s.text(&rows[1]).unwrap().contains("Account 1"));

    // Lock; a wrong passphrase is refused with a message; the right one opens.
    s.click(&button(&s, "Lock")).unwrap();
    let field = s.wait_for("input[type=password]", 1, WAIT).unwrap();
    s.type_text(&field[0], "not the passphrase").unwrap();
    s.click(&button(&s, "Unlock")).unwrap();
    let banner = s.wait_for(".banner.error", 1, WAIT).unwrap();
    println!("wrong passphrase: {}", s.text(&banner[0]).unwrap());
    let field = s.wait_for("input[type=password]", 1, WAIT).unwrap();
    s.replace_text(&field[0], PASSPHRASE).unwrap();
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
    s.end().unwrap();
}
