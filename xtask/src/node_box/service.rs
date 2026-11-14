//! Service definitions for the node, as text. `node setup` prints them; it
//! never installs one. Installing a service is the operator's step, under
//! their own account, so it stays out of an automatic path.

use std::path::Path;

/// The node's arguments for the directory `dir`, in the order the node reads them.
pub fn node_args(dir: &Path, validator_key: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "--genesis".to_owned(),
        dir.join("genesis.json").display().to_string(),
        "--config".to_owned(),
        dir.join("node-config.toml").display().to_string(),
        "--data-dir".to_owned(),
        dir.join("data").display().to_string(),
    ];
    if let Some(key) = validator_key {
        args.push("--validator-key".to_owned());
        args.push(key.display().to_string());
    }
    args
}

/// A systemd unit. Every argument is quoted, so a path with spaces stays one
/// argument. Runs as `user` with the node's own sandbox options.
pub fn systemd_unit(exe: &Path, args: &[String], user: &str) -> String {
    let quoted: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
    format!(
        "# Maya2C node (printed by `cargo xtask node setup`; not installed).\n\
         [Unit]\n\
         Description=Maya2C node\n\
         After=network-online.target\n\
         Wants=network-online.target\n\n\
         [Service]\n\
         Type=simple\n\
         User={user}\n\
         ExecStart=\"{exe}\" {args}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         NoNewPrivileges=yes\n\
         UMask=0077\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        exe = exe.display(),
        args = quoted.join(" "),
    )
}

/// The `schtasks` command that runs the node at boot as SYSTEM. SYSTEM is
/// among the accounts the key rule allows, so the node can read its key.
pub fn windows_task_command(exe: &Path, args: &[String]) -> String {
    let inner: Vec<String> = std::iter::once(format!("\"{}\"", exe.display()))
        .chain(args.iter().map(|a| format!("\"{a}\"")))
        .collect();
    // schtasks wants the whole action as one quoted string, with inner quotes escaped.
    let action = inner.join(" ").replace('"', "\\\"");
    format!(
        "schtasks /Create /TN \"Maya2C node\" /SC ONSTART /RU SYSTEM /RL HIGHEST /TR \"{action}\""
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn node_args_point_into_the_node_directory() {
        // Why: expected paths come from Path::join, so the test holds on Windows and Unix separators alike.
        let dir = Path::new("srv").join("maya2c");
        let key = dir.join("validator.key");
        let args = node_args(&dir, Some(&key));
        assert_eq!(args[0], "--genesis");
        assert!(args.contains(&dir.join("node-config.toml").display().to_string()));
        assert!(args.contains(&dir.join("data").display().to_string()));
        assert_eq!(
            args.last().map(String::as_str),
            Some(key.display().to_string().as_str())
        );
    }

    #[test]
    fn an_observer_has_no_validator_key_argument() {
        let args = node_args(Path::new("/srv/maya2c"), None);
        assert!(!args.iter().any(|a| a == "--validator-key"));
    }

    #[test]
    fn the_unit_quotes_paths_with_spaces_and_runs_as_the_user() {
        let dir = Path::new("srv").join("my node");
        let args = node_args(&dir, None);
        let unit = systemd_unit(Path::new("/usr/local/bin/maya2c-node"), &args, "op");
        assert!(unit.contains("User=op"));
        assert!(unit.contains("ExecStart=\"/usr/local/bin/maya2c-node\" \"--genesis\""));
        let data = dir.join("data").display().to_string();
        assert!(unit.contains(&format!("\"{data}\"")));
        assert!(unit.contains("Restart=on-failure"));
    }

    #[test]
    fn the_windows_task_runs_at_boot_as_system_with_escaped_quotes() {
        let exe = PathBuf::from(r"D:\Maya2C\bin\maya2c-node.exe");
        let args = node_args(Path::new(r"D:\Maya2C-node"), None);
        let line = windows_task_command(&exe, &args);
        assert!(line.starts_with("schtasks /Create"));
        assert!(line.contains("/SC ONSTART /RU SYSTEM"));
        assert!(line.contains(r#"\"D:\Maya2C\bin\maya2c-node.exe\""#));
    }
}
