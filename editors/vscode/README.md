# Maya2C contract debugger for VS Code

A manifest, no extension code: VS Code starts `maya2c dap` (the Rust Debug
Adapter in `bins/maya2c-cli/src/dap.rs`) and talks DAP to it over stdio.

## Install

1. Build the CLI and put it on `PATH`: `cargo build -p maya2c-cli --release`,
   then add `target/release` to `PATH` (or copy `maya2c` somewhere on it).
2. Copy or symlink this folder into your extensions directory as
   `maya2c.maya2c-debug-0.1.0` (`%USERPROFILE%\.vscode\extensions` on
   Windows, `~/.vscode/extensions` elsewhere), and reload VS Code.
3. Add a `maya2c` launch configuration (Run → Add Configuration) naming the
   contract's `.wasm`, the input and optionally the caller.

## What you get

- **Step Over / Step Into** move one host call forward; **Step Back** and
  **Reverse Continue** move backward. Backward is exact: the call ran once
  and is replayed from its recording.
- The frame's source is the recording itself — one line per host call — so
  the editor highlights where you are. **Run to Cursor / Jump to Cursor**
  on a line goes there.
- **Variables:** storage at the cursor, events so far, and the call (step,
  outcome, gas for the whole call).
- **Debug console:** the CLI's commands — `n`, `b`, `g 4`, `s`, `e`, `l`.

## Limits

Steps are host calls, not Rust source lines: contracts carry no DWARF and
the VM has no per-instruction hook. Breakpoints are therefore reported as
unverified. Gas is known for the whole call, not per step. Tested by
`bins/maya2c-cli/tests/dap_tests.rs`, which drives the real binary over
stdio; the extension itself has not been exercised inside VS Code on this
workstation.
