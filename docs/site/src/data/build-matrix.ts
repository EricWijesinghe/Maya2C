// The Quickstart picker: install what building Maya2C needs, build the node
// and wallet, and set up the shell, written for each OS.
//
// The prerequisites come from the repository, not from memory:
// - .cargo/config.toml links x86_64 Linux with clang + mold, and Windows
//   with lld-link (LLVM), so both are required, not optional.
// - RocksDB is compiled in through bindgen, which needs libclang.
// - rust-toolchain.toml pins the nightly. `rustup toolchain install` with no
//   argument installs exactly that pin; newer rustup releases no longer do
//   it implicitly on the first cargo command in every configuration.
// - Windows PowerShell 5.1 aliases `curl` to Invoke-WebRequest, so Windows
//   commands call `curl.exe`.
import {
  comboKey,
  LINUX_DISTRO_HELP,
  LINUX_DISTROS,
  MAC_HELP,
  OS_CHOICES,
  WINDOWS_HELP,
  type Combo,
  type GuideDef,
  type OsId,
  type Step,
} from "./picker";

const REPO = "https://github.com/EricWijesinghe/Maya2C";
const RUSTUP_SH = "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none";
const BUILD = "cargo build -p maya2c-node -p l1-wallet";
const FIRST_BUILD_NOTE =
  "The first build downloads the pinned Rust nightly and compiles RocksDB, so it takes several minutes. Later builds only recompile what changed.";

const linuxPackages: Record<string, string> = {
  debian: "sudo apt update && sudo apt install -y build-essential clang libclang-dev mold pkg-config git curl",
  fedora: "sudo dnf install -y gcc gcc-c++ make clang clang-devel mold pkgconf-pkg-config git curl",
  arch: "sudo pacman -S --needed base-devel clang mold git curl",
  suse: "sudo zypper install -y -t pattern devel_basis && sudo zypper install -y clang clang-devel mold git curl",
};

const rows: GuideDef["rows"] = [
  { name: "os", label: "Your computer", choices: OS_CHOICES },
  {
    name: "v",
    group: "windows",
    label: "Version",
    help: WINDOWS_HELP,
    choices: [
      { id: "11", label: "Windows 11" },
      { id: "10", label: "Windows 10" },
    ],
  },
  {
    name: "v",
    group: "mac",
    label: "Chip",
    help: MAC_HELP,
    choices: [
      { id: "apple", label: "Apple silicon", hint: "M1, M2, M3, M4" },
      { id: "intel", label: "Intel" },
    ],
  },
  { name: "v", group: "linux", label: "Distribution", help: LINUX_DISTRO_HELP, choices: LINUX_DISTROS },
];

const clone: Step = {
  title: "Get the code and build",
  body: [FIRST_BUILD_NOTE],
  command: `git clone ${REPO}\ncd Maya2C\nrustup toolchain install\n${BUILD}`,
};

function windowsSteps(version: string): Step[] {
  const winget: Step[] =
    version === "10"
      ? [
          {
            title: "Make sure winget works",
            body: [
              "Open **PowerShell** (Start › type PowerShell › Enter) and run `winget --version`. If it says the command is not found, install **App Installer** from the Microsoft Store, then open a new PowerShell window.",
            ],
          },
        ]
      : [{ title: "Open Terminal", body: ["Right-click the **Start** button and choose **Terminal**."] }];
  return [
    ...winget,
    {
      title: "Install Git, Rust and LLVM",
      body: ["LLVM provides `lld-link`, the linker this repository uses on Windows, and `libclang` for RocksDB."],
      command: "winget install --id Git.Git -e\nwinget install --id Rustlang.Rustup -e\nwinget install --id LLVM.LLVM -e",
      lang: "powershell",
    },
    {
      title: "Install the C++ build tools",
      body: ["Visual Studio Build Tools with the C++ workload. It is a large download (several GB)."],
      command:
        'winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"',
      lang: "powershell",
    },
    {
      title: "Put LLVM on your PATH",
      body: ["The LLVM installer does not always do this itself. This adds it for your account, then opens a fresh window so every tool is found."],
      command:
        "[Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';C:\\Program Files\\LLVM\\bin', 'User')\nStart-Process powershell; exit",
      lang: "powershell",
    },
    { ...clone, lang: "powershell" },
    {
      title: "Use the binaries from any folder",
      body: ["For this window only. Run it from the `Maya2C` folder."],
      command: '$env:Path = "$PWD\\target\\debug;$env:Path"',
      lang: "powershell",
      after: "Check: `maya2c-node --version` prints a version.",
    },
  ];
}

function unixShellSteps(): Step[] {
  return [
    { ...clone, lang: "bash" },
    {
      title: "Use the binaries from any folder",
      body: ["For this terminal only. Run it from the `Maya2C` folder."],
      command: 'export PATH="$PWD/target/debug:$PATH"',
      lang: "bash",
      after: "Check: `maya2c-node --version` prints a version.",
    },
  ];
}

function macSteps(): Step[] {
  return [
    { title: "Open Terminal", body: ["Press **⌘ Command + Space**, type **Terminal**, and press **Return**."] },
    {
      title: "Install Apple's command-line tools",
      body: ["A window asks to install them; click **Install**. If it says they are already installed, go on."],
      command: "xcode-select --install",
      lang: "bash",
    },
    {
      title: "Install Rust",
      body: ["Then load it into this terminal:"],
      command: `${RUSTUP_SH}\n. "$HOME/.cargo/env"`,
      lang: "bash",
    },
    ...unixShellSteps(),
  ];
}

function linuxSteps(distro: string): Step[] {
  return [
    { title: "Open a terminal", body: ["Usually **Ctrl + Alt + T**, or search for **Terminal** in your apps."] },
    {
      title: "Install the build tools",
      body: ["`clang` and `mold` are required: this repository links with mold on x86_64 Linux."],
      command: linuxPackages[distro],
      lang: "bash",
    },
    {
      title: "Install Rust",
      body: ["Then load it into this terminal:"],
      command: `${RUSTUP_SH}\n. "$HOME/.cargo/env"`,
      lang: "bash",
    },
    ...unixShellSteps(),
  ];
}

function stepsFor(os: OsId, version: string): Step[] {
  if (os === "windows") return windowsSteps(version);
  if (os === "mac") return macSteps();
  return linuxSteps(version);
}

const TABS = { windows: "Windows", mac: "Mac", linux: "Linux" } as const;

function allCombos(): Combo[] {
  return rows
    .filter((r) => r.name === "v")
    .flatMap((r) =>
      r.choices.map((c) => ({
        key: comboKey(r.group!, c.id),
        w: { tab: TABS[r.group!], steps: stepsFor(r.group!, c.id) },
      })),
    );
}

/** The Quickstart picker: build from source. */
export const buildGuide: GuideDef = {
  rows,
  defaultKey: comboKey("windows", "11"),
  lastStepNote: 'Last step. Go on to "2. A validator key and a wallet" below.',
  combos: allCombos(),
};
