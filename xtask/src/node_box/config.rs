//! `node-config.toml`: the template is the node's own output
//! (`maya2c-node --print-config-template`), so it cannot drift from the
//! defaults the node reads. An existing file is kept unless `--force`.

use std::path::Path;

use crate::devnet::Result;

/// Sections the node requires; `node doctor` checks they are present.
pub const REQUIRED_SECTIONS: [&str; 2] = ["network", "rpc"];

#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    Created,
    Replaced,
    Kept,
}

/// Writes `template` to `path`. Never overwrites a file unless `force`.
///
/// # Errors
///
/// The file cannot be written.
pub fn write_config(path: &Path, template: &str, force: bool) -> Result<Written> {
    let existed = path.exists();
    if existed && !force {
        return Ok(Written::Kept);
    }
    std::fs::write(path, template).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(if existed {
        Written::Replaced
    } else {
        Written::Created
    })
}

/// The required sections missing from `text`, or the parse error. An empty
/// list means the file has every section the node needs.
///
/// # Errors
///
/// The text is not TOML.
pub fn missing_sections(text: &str) -> Result<Vec<&'static str>> {
    let value: toml::Value = toml::from_str(text).map_err(|e| format!("not valid TOML: {e}"))?;
    Ok(REQUIRED_SECTIONS
        .into_iter()
        .filter(|s| value.get(*s).is_none())
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A fresh directory under the temp dir, unique to this test.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const TEMPLATE: &str = "[network]\nbootnodes = []\n\n[rpc]\nrate_limit_per_second = 50\n";

    #[test]
    fn a_missing_config_is_created_from_the_template() {
        let dir = scratch("created");
        let path = dir.join("node-config.toml");
        assert_eq!(
            write_config(&path, TEMPLATE, false).unwrap(),
            Written::Created
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
    }

    #[test]
    fn an_existing_config_is_kept_unless_forced() {
        let dir = scratch("kept");
        let path = dir.join("node-config.toml");
        std::fs::write(&path, "[operator]\nnote = \"mine\"\n").unwrap();
        assert_eq!(write_config(&path, TEMPLATE, false).unwrap(), Written::Kept);
        assert!(std::fs::read_to_string(&path).unwrap().contains("mine"));
        assert_eq!(
            write_config(&path, TEMPLATE, true).unwrap(),
            Written::Replaced
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
    }

    #[test]
    fn a_config_with_every_section_has_nothing_missing() {
        assert!(missing_sections(TEMPLATE).unwrap().is_empty());
    }

    #[test]
    fn a_config_without_rpc_names_the_missing_section() {
        assert_eq!(
            missing_sections("[network]\nbootnodes = []\n").unwrap(),
            ["rpc"]
        );
    }

    #[test]
    fn text_that_is_not_toml_is_an_error() {
        assert!(missing_sections("[network\n").is_err());
    }
}
