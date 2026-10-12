// What the install picker on /guides/help-run offers, and the steps for every
// combination of choices. Like the CUDA download picker: choose the computer,
// its version or chip, and how to install, and the walkthrough changes.
//
// Every command here is one that scripts/join-testnet.sh,
// scripts/join-testnet.ps1 or infra/testnet-vm/join-validator.ps1 actually
// accepts, and every "not yet" matches a build missing from the newest
// testnet release (node-v0.1.0-testnet.3: Windows x64, macOS Apple silicon,
// Linux x86_64 and ARM64). Add a choice here only when its build ships.

import {
  comboKey,
  LINUX_DISTRO_HELP,
  LINUX_DISTROS,
  MAC_HELP,
  OS_CHOICES,
  WINDOWS_HELP,
  type Choice,
  type Combo,
  type GuideDef,
  type OsId,
  type Step,
  type Walkthrough,
} from "./picker";

type MethodId = "oneline" | "manual";

const RAW = "https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master";
const PS_ONE_LINE = `irm ${RAW}/scripts/join-testnet.ps1 | iex`;
const SH_ONE_LINE = `curl -fsSL ${RAW}/scripts/join-testnet.sh | bash`;
const PS_FILE = "$env:USERPROFILE\\Downloads\\join-validator.ps1";

const methodChoices: Choice[] = [
  { id: "oneline", label: "One line", hint: "recommended" },
  { id: "manual", label: "Download, read, then run", hint: "for the cautious" },
];

const archChoices: Choice[] = [
  { id: "x86_64", label: "x86_64 (Intel / AMD)" },
  { id: "arm64", label: "ARM64", hint: "Raspberry Pi 4/5 with a 64-bit OS, ARM servers" },
];

const rows: GuideDef["rows"] = [
  { name: "os", label: "Your computer", choices: OS_CHOICES },
  {
    name: "v",
    group: "windows",
    label: "Version",
    help: WINDOWS_HELP,
    choices: [
      { id: "11", label: "Windows 11" },
      { id: "10", label: "Windows 10", hint: "version 1803 or newer" },
      { id: "arm", label: "Windows on ARM", hint: "Surface Pro X, Snapdragon laptops" },
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
  {
    name: "arch",
    group: "linux",
    label: "Processor",
    help: "Not sure? Run `uname -m`. `x86_64` is Intel / AMD; `aarch64` is ARM64. `armv7l` means a 32-bit OS, which has no build.",
    choices: archChoices,
  },
  { name: "m", label: "How to install", choices: methodChoices },
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

function walkthrough(os: OsId, version: string, method: MethodId): Walkthrough {
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

function allCombos(): Combo[] {
  const out: Combo[] = [];
  for (const row of rows.filter((r) => r.name === "v")) {
    const os = row.group!;
    const arches = os === "linux" ? archChoices.map((a) => a.id) : ["-"];
    for (const v of row.choices) {
      for (const a of arches) {
        for (const m of methodChoices) {
          out.push({ key: comboKey(os, v.id, a, m.id), w: walkthrough(os, v.id, m.id as MethodId) });
        }
      }
    }
  }
  return out;
}

/** The "Help run the network" picker: install a testnet node. */
export const joinGuide: GuideDef = {
  rows,
  defaultKey: comboKey("windows", "11", "-", "oneline"),
  lastStepNote: 'Last step. Then go on to "Check it is working" below.',
  combos: allCombos(),
};
