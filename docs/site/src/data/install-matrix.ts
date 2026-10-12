// What the install picker on /guides/help-run offers, and the steps for every
// combination of choices. Like the CUDA download picker: choose the computer,
// its version or chip, and how to install, and the walkthrough changes.
//
// Every command here is one that scripts/join-testnet.sh,
// scripts/join-testnet.ps1 or infra/testnet-vm/join-validator.ps1 actually
// accepts, and every "not yet" matches a build missing from the newest
// testnet release (node-v0.1.0-testnet.3: Windows x64, macOS Apple silicon,
// Linux x86_64 and ARM64). Add a choice here only when its build ships.

export type OsId = "windows" | "mac" | "linux";
export type MethodId = "oneline" | "manual";

export interface Choice {
  id: string;
  label: string;
  hint?: string;
}

export interface Step {
  title: string;
  /** Plain sentences; `**bold**` and `` `code` `` are rendered. */
  body?: string[];
  /** A command the reader copies. */
  command?: string;
  lang?: "powershell" | "bash";
  /** Shown after the command: what the reader should see next. */
  after?: string;
}

export interface Walkthrough {
  /** Set when there is no build for this combination yet. */
  unavailable?: string;
  steps: Step[];
  /** The tab label the rest of the page uses for this OS. */
  tab: "Windows" | "Mac" | "Linux";
}

const RAW = "https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master";
const PS_ONE_LINE = `irm ${RAW}/scripts/join-testnet.ps1 | iex`;
const SH_ONE_LINE = `curl -fsSL ${RAW}/scripts/join-testnet.sh | bash`;
const PS_FILE = "$env:USERPROFILE\\Downloads\\join-validator.ps1";

export const osChoices: Choice[] = [
  { id: "windows", label: "Windows" },
  { id: "mac", label: "macOS" },
  { id: "linux", label: "Linux" },
];

/** The second row: Windows edition, Mac chip, or Linux distribution. */
export const versionChoices: Record<OsId, { label: string; choices: Choice[]; help: string }> = {
  windows: {
    label: "Version",
    help: "Not sure? Press Windows key + R, type `winver`, press Enter.",
    choices: [
      { id: "11", label: "Windows 11" },
      { id: "10", label: "Windows 10", hint: "version 1803 or newer" },
      { id: "arm", label: "Windows on ARM", hint: "Surface Pro X, Snapdragon laptops" },
    ],
  },
  mac: {
    label: "Chip",
    help: "Not sure? Apple menu  › About This Mac. \"Chip: Apple M…\" is Apple silicon; \"Processor: Intel…\" is Intel.",
    choices: [
      { id: "apple", label: "Apple silicon", hint: "M1, M2, M3, M4" },
      { id: "intel", label: "Intel" },
    ],
  },
  linux: {
    label: "Distribution",
    help: "Not sure? Run `cat /etc/os-release` and look at the NAME line.",
    choices: [
      { id: "debian", label: "Ubuntu / Debian", hint: "also Mint, Pop!_OS, Raspberry Pi OS" },
      { id: "fedora", label: "Fedora / RHEL", hint: "also Rocky, Alma, CentOS Stream" },
      { id: "arch", label: "Arch", hint: "also Manjaro, EndeavourOS" },
      { id: "suse", label: "openSUSE" },
    ],
  },
};

export const archChoices: Choice[] = [
  { id: "x86_64", label: "x86_64 (Intel / AMD)" },
  { id: "arm64", label: "ARM64", hint: "Raspberry Pi 4/5 with a 64-bit OS, ARM servers" },
];
export const archHelp = "Not sure? Run `uname -m`. `x86_64` is Intel / AMD; `aarch64` is ARM64. `armv7l` means a 32-bit OS, which has no build.";

export const methodChoices: Choice[] = [
  { id: "oneline", label: "One line", hint: "recommended" },
  { id: "manual", label: "Download, read, then run", hint: "for the cautious" },
];

const curlInstall: Record<string, string> = {
  debian: "sudo apt update && sudo apt install -y curl",
  fedora: "sudo dnf install -y curl",
  arch: "sudo pacman -S --needed curl",
  suse: "sudo zypper install -y curl",
};

function openPowerShell(version: string): Step {
  return version === "10"
    ? {
        title: "Open PowerShell",
        body: ["Click **Start**, type **PowerShell**, and press **Enter**. A blue window opens."],
      }
    : {
        title: "Open Terminal",
        body: [
          "Right-click the **Start** button and choose **Terminal**. A window opens with PowerShell inside it.",
        ],
      };
}

const windowsFinish: Step[] = [
  {
    title: "Click Yes",
    body: ["Windows asks **\"Do you want to allow this app to make changes?\"** Click **Yes**."],
  },
  {
    title: "Confirm the install",
    body: [
      "A second window lists everything it will install and asks **Install? [y/N]**. Type `y` and press **Enter**.",
      "If **\"Windows protected your PC\"** appears, click **More info**, then **Run anyway**. The program is not code-signed yet; every download is checked against its published fingerprint before it runs.",
    ],
  },
  {
    title: "Wait for Done",
    body: [
      "Wait for **Done. Thank you for helping!** Then close the windows. The node keeps running in the background and starts by itself with Windows.",
    ],
  },
];

function windowsSteps(version: string, method: MethodId): Step[] {
  if (method === "oneline") {
    return [
      openPowerShell(version),
      {
        title: "Paste the install line",
        body: ["Copy this line, paste it into the window (right-click pastes), and press **Enter**:"],
        command: PS_ONE_LINE,
        lang: "powershell",
      },
      ...windowsFinish,
    ];
  }
  return [
    openPowerShell(version),
    {
      title: "Download the installer",
      body: ["This saves the installer to your **Downloads** folder without running it:"],
      command: `iwr ${RAW}/infra/testnet-vm/join-validator.ps1 -OutFile ${PS_FILE}`,
      lang: "powershell",
    },
    {
      title: "Read it (optional)",
      body: ["Opens the installer in Notepad. Its first screen says in plain words everything it will do."],
      command: `notepad ${PS_FILE}`,
      lang: "powershell",
    },
    {
      title: "Run it as Administrator",
      body: ["Starts the installer in an Administrator window:"],
      command: `Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile','-NoExit','-ExecutionPolicy','Bypass','-File',"${PS_FILE}"`,
      lang: "powershell",
      after: "To only preview what it would do, without changing anything, add `,'-DryRun'` to the very end of the line.",
    },
    ...windowsFinish,
  ];
}

const macOpenTerminal: Step = {
  title: "Open Terminal",
  body: ["Press **⌘ Command + Space**, type **Terminal**, and press **Return**."],
};

const macFinish: Step[] = [
  {
    title: "Type your Mac password",
    body: [
      "When it asks for **Password**, type your Mac login password and press **Return**. **Nothing shows while you type**; that is normal.",
    ],
  },
  {
    title: "Wait for Done",
    body: [
      "Wait for **Done. Thank you for helping!** Then close Terminal. The node runs in the background and starts by itself with the Mac.",
      "Keep the Mac **plugged in with the lid open**. For extra safety: **System Settings › Battery › Options** › turn on **Prevent automatic sleeping on power adapter when the display is off**.",
    ],
  },
];

function macSteps(method: MethodId): Step[] {
  if (method === "oneline") {
    return [
      macOpenTerminal,
      {
        title: "Paste the install line",
        body: ["Copy this line, paste it into Terminal (**⌘ V**), and press **Return**:"],
        command: SH_ONE_LINE,
        lang: "bash",
      },
      ...macFinish,
    ];
  }
  return [
    macOpenTerminal,
    {
      title: "Download the installer",
      body: ["Saves the installer in your home folder without running it:"],
      command: `cd ~ && curl -fsSLO ${RAW}/scripts/join-testnet.sh`,
      lang: "bash",
    },
    {
      title: "Read it (optional)",
      body: ["Opens it in TextEdit. The comment at the top lists everything it does."],
      command: "open -e ~/join-testnet.sh",
      lang: "bash",
    },
    { title: "Run it", command: "bash ~/join-testnet.sh", lang: "bash" },
    ...macFinish,
  ];
}

function linuxSteps(distro: string, method: MethodId): Step[] {
  const prep: Step[] = [
    { title: "Open a terminal", body: ["Usually **Ctrl + Alt + T**, or search for **Terminal** in your apps."] },
    {
      title: "Make sure curl is installed",
      body: ["Most systems already have it; running this again does no harm."],
      command: curlInstall[distro],
      lang: "bash",
    },
  ];
  const finish: Step[] = [
    {
      title: "Wait for Done",
      body: [
        "Type your password if `sudo` asks for it. Wait for **Done. Thank you for helping!**",
        "The node runs as the `maya2c-node` systemd service and starts with the computer.",
      ],
    },
  ];
  if (method === "oneline") {
    return [
      ...prep,
      { title: "Paste the install line", command: SH_ONE_LINE, lang: "bash" },
      ...finish,
    ];
  }
  return [
    ...prep,
    {
      title: "Download the installer",
      command: `cd ~ && curl -fsSLO ${RAW}/scripts/join-testnet.sh`,
      lang: "bash",
    },
    {
      title: "Read it (optional)",
      body: ["Press **q** to leave the viewer."],
      command: "less ~/join-testnet.sh",
      lang: "bash",
    },
    { title: "Run it", command: "bash ~/join-testnet.sh", lang: "bash" },
    ...finish,
  ];
}

/** One key per combination, e.g. `linux.fedora.arm64.manual`. */
export function comboKey(os: OsId, version: string, arch: string, method: MethodId): string {
  return [os, version, os === "linux" ? arch : "-", method].join(".");
}

export function walkthrough(os: OsId, version: string, arch: string, method: MethodId): Walkthrough {
  if (os === "windows") {
    if (version === "arm") {
      return {
        tab: "Windows",
        steps: [],
        unavailable:
          "There is no Windows on ARM build yet. Windows' x64 emulation may run the x64 build, but nobody has tested that. Tell Eric if you want to try it.",
      };
    }
    return { tab: "Windows", steps: windowsSteps(version, method) };
  }
  if (os === "mac") {
    if (version === "intel") {
      return {
        tab: "Mac",
        steps: [],
        unavailable:
          "There is no Intel Mac build in a release yet. It arrives with the next release; the installer says so too, so it cannot install the wrong thing.",
      };
    }
    return { tab: "Mac", steps: macSteps(method) };
  }
  return { tab: "Linux", steps: linuxSteps(version, method) };
}

/** Every combination, so the page can render them all at build time. */
export function allCombos(): { key: string; os: OsId; w: Walkthrough }[] {
  const out: { key: string; os: OsId; w: Walkthrough }[] = [];
  const methods = methodChoices.map((m) => m.id as MethodId);
  for (const os of osChoices.map((o) => o.id as OsId)) {
    const arches = os === "linux" ? archChoices.map((a) => a.id) : ["-"];
    for (const v of versionChoices[os].choices.map((c) => c.id)) {
      for (const a of arches) {
        for (const m of methods) {
          out.push({ key: comboKey(os, v, a, m), os, w: walkthrough(os, v, a, m) });
        }
      }
    }
  }
  return out;
}
