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
#[cfg(test)]
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
#[cfg(test)]
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
