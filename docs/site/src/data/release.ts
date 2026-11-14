// The one tagged pre-release every download link on the site points at
// (lane L04). Links name the tag and its exact asset names, never GitHub's
// moving "latest" alias: "latest" moves, and a guide that says "this file" must
// keep meaning that file.
//
// `published` stays false until the release exists on GitHub. Publishing needs
// the owner's APPROVED (Standing Order 6), and until then a download button
// would be a 404, so the guide shows one "not released yet" card instead. When it is
// published: copy each asset name from
//   gh release view <tag> --json assets --jq '.assets[].name'
// into `wallet` below and set `published: true`. paused-truth.test.ts fails if
// a published release is missing an asset name.
const repo = "https://github.com/EricWijesinghe/Maya2C";

export const release = {
  tag: "node-v0.2.0-preview.1",
  published: false,
  /** Release notes committed with the tag; they exist before the release does. */
  notes: `${repo}/blob/master/docs/releases/node-v0.2.0-preview.1.md`,
} as const;

export type WalletOs = "windows" | "macos" | "linux";

/** Exact asset names on the release, per OS. Null until the release is published. */
export const wallet: Record<WalletOs, string | null> = {
  windows: null,
  macos: null,
  linux: null,
};

export const walletOsLabel: Record<WalletOs, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };

/** The tag's release page once published; the committed notes before that. */
export const releaseHref = release.published ? `${repo}/releases/tag/${release.tag}` : release.notes;

/** A download URL for one asset of the tagged release. */
export function assetHref(name: string): string {
  return `${repo}/releases/download/${release.tag}/${encodeURIComponent(name)}`;
}

/** Why a download button is off, or null when the release is out. */
export const unpublishedReason: string | null = release.published
  ? null
  : `${release.tag} is not released yet, so there is no installer to download. The wallet builds from source today, and the notes are already readable.`;

/** Where the wallet's build-from-source steps are, while there is no installer. */
export const BUILD_FROM_SOURCE_HREF = "/guides/quickstart/#1-install-the-tools-and-build";
