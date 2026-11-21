//! `L1_WALLET_PASSWORD_FILE`: scripted use without the password in the
//! process environment. The file wins over `L1_WALLET_PASSWORD`.

use std::process::{Command, Stdio};

use l1_wallet::keystore;

const FILE_PASSWORD: &str = "password-from-the-file";
const ENV_PASSWORD: &str = "password-from-the-environment";

#[test]
fn generate_takes_the_password_from_the_file_before_the_environment() {
    let dir = std::env::temp_dir().join(format!("l1w-password-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    let pass = dir.join("pass");
    // One trailing newline is dropped, as every passphrase file in the tree is read.
    std::fs::write(&pass, format!("{FILE_PASSWORD}\n")).expect("write the password file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pass, std::fs::Permissions::from_mode(0o600))
            .expect("make the password file owner-only");
    }
    let ks = dir.join("wallet.key");

    let output = Command::new(env!("CARGO_BIN_EXE_l1-wallet"))
        .arg("--keystore")
        .arg(&ks)
        .arg("generate")
        .env("L1_WALLET_PASSWORD_FILE", &pass)
        .env("L1_WALLET_PASSWORD", ENV_PASSWORD)
        .stdin(Stdio::null())
        .output()
        .expect("run l1-wallet");
    assert!(
        output.status.success(),
        "generate: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let blob = std::fs::read(&ks).expect("the keystore was written");
    let opened_with_file = keystore::decrypt(&blob, FILE_PASSWORD).is_ok();
    let opened_with_env = keystore::decrypt(&blob, ENV_PASSWORD).is_ok();
    std::fs::remove_dir_all(&dir).expect("remove the test directory");
    assert!(opened_with_file, "the file's password does not open the keystore");
    assert!(!opened_with_env, "the environment password was used over the file");
}
