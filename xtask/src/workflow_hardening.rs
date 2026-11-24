//! Regression guards for the CI workflow hardening. Each test reads a committed
//! workflow or config and fails when a permission, gate or pin it relies on is
//! dropped. xtask has no YAML parser, so blocks are read by indentation, which
//! these files keep consistent.

const PUBLISH_SDK: &str = include_str!("../../.github/workflows/publish_sdk.yml");
const RELEASE_BINARIES: &str = include_str!("../../.github/workflows/release-binaries.yml");
const SITE_LINKS: &str = include_str!("../../.github/workflows/site-links.yml");
const DEPENDABOT: &str = include_str!("../../.github/dependabot.yml");

/// Directories holding a Cargo.lock or package-lock.json outside the root
/// workspace and docs/site, each of which needs its own Dependabot entry.
const NESTED_LOCKFILE_DIRS: &[&str] = &[
    "/apps/chat-gui/src-tauri",
    "/apps/dashboard",
    "/apps/hub/src-tauri",
    "/apps/ledger-maya2c",
    "/apps/wallet-gui/src-tauri",
    "/apps/wallet-gui/ui",
    "/contracts/nft-game",
    "/contracts/token-swap",
    "/fuzz",
    "/hal/ebpf-net/programs",
    "/hal/iot-firmware",
    "/offsec-sandbox",
    "/sdks/sdk-js",
];

/// The lines of the block opened by `header` (exact text, with its indent),
/// up to the next key at the same or a shallower indent.
fn block(yaml: &str, header: &str) -> String {
    let indent = header.len() - header.trim_start().len();
    let mut lines = Vec::new();
    let mut inside = false;
    for line in yaml.lines() {
        if !inside {
            inside = line == header;
            continue;
        }
        let content = line.trim_start();
        let depth = line.len() - content.len();
        if !content.is_empty() && !content.starts_with('#') && depth <= indent {
            break;
        }
        lines.push(line);
    }
    assert!(inside, "block {header:?} not found");
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_grants_no_oidc_token_by_default() {
        assert!(!block(PUBLISH_SDK, "permissions:").contains("id-token"));
    }

    #[test]
    fn only_the_publish_jobs_mint_an_oidc_token() {
        assert!(block(PUBLISH_SDK, "  npm:").contains("id-token: write"));
        assert!(block(PUBLISH_SDK, "  pypi:").contains("id-token: write"));
        // The wheel build runs third-party build code and must not hold a token.
        assert!(!block(PUBLISH_SDK, "  wheels:").contains("id-token"));
    }

    #[test]
    fn npm_publish_binds_to_its_environment() {
        assert!(block(PUBLISH_SDK, "  npm:").contains("environment: npm"));
    }

    #[test]
    fn publish_steps_run_only_for_version_tags() {
        for job in ["  npm:", "  pypi:"] {
            let body = block(PUBLISH_SDK, job);
            assert!(
                body.contains("if: startsWith(github.ref, 'refs/tags/v')"),
                "{job} publish is not gated to version tags"
            );
        }
    }

    #[test]
    fn publish_triggers_only_on_semver_tags() {
        let on = block(PUBLISH_SDK, "on:");
        assert!(on.contains(r#"- "v[0-9]+.[0-9]+.[0-9]+*""#), "{on}");
        assert!(!on.contains(r#"- "v*""#), "any v* tag would publish");
    }

    #[test]
    fn wheel_build_installs_a_checksummed_rustup() {
        let wheels = block(PUBLISH_SDK, "  wheels:");
        assert!(!wheels.contains("sh.rustup.rs"), "rustup script piped to sh");
        assert!(wheels.contains("static.rust-lang.org/rustup/archive/"));
        assert!(wheels.contains("sha256sum -c"));
    }

    #[test]
    fn wheel_package_dir_exists() {
        let wheels = block(PUBLISH_SDK, "  wheels:");
        let dir = wheels
            .lines()
            .find_map(|l| l.trim().strip_prefix("package-dir: "))
            .expect("no package-dir");
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(dir)
            .join("pyproject.toml");
        assert!(manifest.exists(), "{} missing", manifest.display());
    }

    #[test]
    fn wheel_version_must_match_the_tag() {
        assert!(block(PUBLISH_SDK, "  wheels:").contains("sdks/sdk-ffi/pyproject.toml"));
    }

    #[test]
    fn pypi_wheels_get_build_provenance() {
        let pypi = block(PUBLISH_SDK, "  pypi:");
        assert!(pypi.contains("attestations: write"));
        assert!(pypi.contains("uses: actions/attest-build-provenance@"));
        assert!(pypi.contains("subject-path: dist/*.whl"));
    }

    #[test]
    fn build_checkouts_do_not_persist_the_token() {
        for job in ["  build:", "  wallet:", "  chat:", "  hub:"] {
            assert!(
                block(RELEASE_BINARIES, job).contains("persist-credentials: false"),
                "{job} checkout keeps the token in .git/config"
            );
        }
    }

    #[test]
    fn tauri_builds_check_the_lockfile_first() {
        for job in ["  wallet:", "  chat:"] {
            assert!(
                block(RELEASE_BINARIES, job).contains("cargo metadata --locked"),
                "{job} tauri build is not preceded by a lockfile check"
            );
        }
    }

    /// The job's lines without comments, so prose that names a tool does not count.
    fn code(yaml: &str, header: &str) -> String {
        block(yaml, header)
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // APP-07: the job that can mint Sigstore and attestation identities must not
    // compile repository or dependency code (build scripts, proc macros).
    #[test]
    fn sign_job_runs_no_repository_or_dependency_code() {
        let sign = code(RELEASE_BINARIES, "  sign:");
        assert!(sign.contains("id-token: write"), "sign job no longer signs");
        for banned in [
            "actions/checkout",
            "rust-toolchain",
            "cargo ",
            "install-action",
        ] {
            assert!(!sign.contains(banned), "sign job runs {banned:?}");
        }
    }

    #[test]
    fn sbom_is_built_without_signing_rights() {
        let sbom = code(RELEASE_BINARIES, "  sbom:");
        assert!(sbom.contains("cargo cyclonedx"));
        assert!(!sbom.contains("id-token") && !sbom.contains("environment:"));
        // The `needs:` line itself: the sign job's comments also name `sbom`.
        assert!(
            code(RELEASE_BINARIES, "  sign:")
                .lines()
                .any(|l| l.trim_start().starts_with("needs:") && l.contains("sbom")),
            "sign does not wait for the SBOM"
        );
    }

    #[test]
    fn release_self_check_runs_outside_the_signing_job() {
        let verify = code(RELEASE_BINARIES, "  verify:");
        assert!(verify.contains("cargo run --locked -p xtask -- verify-release"));
        assert!(!verify.contains("id-token") && !verify.contains("attestations: write"));
        assert!(!verify.contains("contents: write"));
        // `gh attestation verify` reads the attestations API with this token.
        assert!(verify.contains("attestations: read"));
        assert!(block(RELEASE_BINARIES, "  release:").contains("needs: [verify]"));
    }

    // APP-08: the in-tree half of the gate. The environment's reviewers and tag
    // rule live in repository settings (recorded in docs/verify-release.md); these pin
    // that every key- or write-holding job asks for that environment at all.
    #[test]
    fn privileged_release_jobs_bind_to_the_release_environment() {
        assert!(block(RELEASE_BINARIES, "  push:").contains("tags: [\"node-v*\"]"));
        for job in ["  updater-sign:", "  sign:", "  release:"] {
            let body = code(RELEASE_BINARIES, job);
            assert!(
                body.contains("environment: release"),
                "{job} skips the release environment"
            );
            assert!(
                body.contains("if: startsWith(github.ref, 'refs/tags/node-v')"),
                "{job} runs outside node-v* tags"
            );
        }
    }

    #[test]
    fn site_links_install_without_lifecycle_scripts() {
        assert!(SITE_LINKS.contains("npm ci --ignore-scripts"));
        assert!(SITE_LINKS.contains("persist-credentials: false"));
    }

    #[test]
    fn every_nested_lockfile_has_a_dependabot_entry() {
        for dir in NESTED_LOCKFILE_DIRS {
            assert!(
                DEPENDABOT.contains(&format!("directory: \"{dir}\"")),
                "no Dependabot entry for {dir}"
            );
        }
    }
}
