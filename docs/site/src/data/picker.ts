// Shared shape of the OS pickers (components/InstallPicker.astro). Each guide
// lists its rows of choices and builds a walkthrough for every combination;
// the component renders them all at build time and shows one.

export type OsId = "windows" | "mac" | "linux";

export interface Choice {
  id: string;
  label: string;
  hint?: string;
}

/**
 * One row of buttons. `name` is the URL parameter it sets. A row with a
 * `group` shows only while that OS is chosen (Windows edition, Mac chip,
 * Linux distribution).
 */
export interface RowDef {
  name: "os" | "v" | "arch" | "m";
  label: string;
  choices: Choice[];
  group?: OsId;
  help?: string;
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

export interface Combo {
  key: string;
  w: Walkthrough;
}

export interface GuideDef {
  rows: RowDef[];
  defaultKey: string;
  /** Shown under the last step. */
  lastStepNote: string;
  combos: Combo[];
}

/** One key per combination, e.g. `linux.fedora.arm64.manual`; `-` for a row the guide or OS lacks. */
export function comboKey(os: OsId, version: string, arch = "-", method = "-"): string {
  return [os, version, arch, method].join(".");
}

export const OS_CHOICES: Choice[] = [
  { id: "windows", label: "Windows" },
  { id: "mac", label: "macOS" },
  { id: "linux", label: "Linux" },
];

export const TAB_OF: Record<OsId, Walkthrough["tab"]> = { windows: "Windows", mac: "Mac", linux: "Linux" };

export const LINUX_DISTROS: Choice[] = [
  { id: "debian", label: "Ubuntu / Debian", hint: "also Mint, Pop!_OS, Raspberry Pi OS" },
  { id: "fedora", label: "Fedora / RHEL", hint: "also Rocky, Alma, CentOS Stream" },
  { id: "arch", label: "Arch", hint: "also Manjaro, EndeavourOS" },
  { id: "suse", label: "openSUSE" },
];
export const LINUX_DISTRO_HELP = "Not sure? Run `cat /etc/os-release` and look at the NAME line.";
export const WINDOWS_HELP = "Not sure? Press Windows key + R, type `winver`, press Enter.";
export const MAC_HELP =
  'Not sure? Apple menu  › About This Mac. "Chip: Apple M…" is Apple silicon; "Processor: Intel…" is Intel.';
