// Copy `docs/*.md` into Starlight's content collection.
//
// # Why a copy and not a move
//
// `docs/*.md` is where these are read. README.md, docs/README.md, CLAUDE.md and
// the docs' own cross-references all link to `docs/foo.md`, and GitHub renders
// them there. Moving them into `src/content/docs/` would break every one of
// those links to gain a site that is a second way to read the same text.
//
// So the markdown stays put and this generates the site's copy. The generated
// tree is gitignored: two copies in version control is two things to keep in
// step, and only one of them is authored.
//
// # Frontmatter is derived, not authored
//
// Starlight requires a `title`. None of these files have frontmatter -- they
// open with an `# H1`, because that is what renders on GitHub. Adding
// frontmatter to the sources would put YAML at the top of twenty files to serve
// a build step, and GitHub would render it as a paragraph of stray text.
//
// So the H1 becomes the title and is stripped from the body, which is what
// Starlight expects: it renders the title itself from frontmatter, and leaving
// the H1 in place produces the heading twice.

import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const DOCS = join(here, "..", "..");
const OUT = join(here, "..", "src", "content", "docs");

/** Files that are not reference pages and get no generated copy. */
const SKIP = new Set([
  // The index becomes the sidebar, not a page.
  "README.md",
]);

/**
 * Hand-written pages, copied in alongside the generated ones.
 *
 * They live in `authored/` rather than in `src/content/docs/` because this
 * script wipes its output directory on every run -- and the first version put
 * them inside it, so a build deleted three pages that nothing else had a copy
 * of. The output tree is generated in full; anything authored belongs outside
 * it, in version control.
 */
const AUTHORED_SRC = join(here, "..", "authored");
const AUTHORED_OUT = "guides";

/**
 * The landing page served at the site root (maya2c.dev/).
 *
 * Kept beside `authored/` rather than in it: that directory is copied into
 * `guides/`, and the home page must land at the collection root instead.
 */
const HOME_SRC = join(here, "..", "home.mdx");

/**
 * Subdirectory the generated reference pages land in.
 *
 * Not the collection root, and this is a workaround rather than a preference:
 * top-level `.md` files whose names match a document in `docs/` were silently
 * excluded from the build -- byte-identical copies under any other name built
 * fine, and so did the same bytes inside a subdirectory. The cause was not
 * found. Ruled out: the sidebar, `.gitignore`, mtime, the derived frontmatter,
 * and the Astro content cache.
 *
 * A subdirectory is where these belong anyway -- `/reference/dex` reads better
 * than `/dex` beside `/guides/api` -- so the workaround costs nothing. If the
 * top level ever starts working, moving them back is one constant.
 */
const REFERENCE_OUT = "reference";

/**
 * Splits an H1 title off the front of a document.
 *
 * Returns the title and the remaining body. A file with no H1 keeps its whole
 * body and reports no title -- the caller decides what to do, rather than this
 * inventing one from a filename.
 */
function splitTitle(markdown) {
  const lines = markdown.split(/\r?\n/);
  const index = lines.findIndex((line) => line.startsWith("# "));
  if (index === -1) return { title: null, body: markdown };

  const title = lines[index].slice(2).trim();
  lines.splice(index, 1);
  // Drop the blank line the H1 left behind, so the body does not start with a
  // gap under Starlight's own rendered title.
  while (lines.length > 0 && lines[0].trim() === "") lines.shift();
  return { title, body: lines.join("\n") };
}

/**
 * YAML-safe scalar.
 *
 * Titles here contain colons ("Network telemetry: …" is one edit away), and an
 * unquoted colon in YAML is a mapping. Single quotes with doubling is the
 * simplest form that cannot be broken by punctuation.
 */
function yamlString(value) {
  return `'${value.replace(/'/g, "''")}'`;
}

/**
 * Rewrites links between docs so they resolve as site routes.
 *
 * On GitHub `[dex](dex.md)` is correct. Starlight serves `/dex/`, so the
 * extension has to go -- otherwise every cross-reference 404s on the site while
 * looking perfectly fine in the repository.
 */
function rewriteLinks(body) {
  return body
    // Sibling docs: (foo.md) -> (/foo)
    .replace(
      /\]\((?!https?:)([A-Za-z0-9_-]+)\.md(#[^)]*)?\)/g,
      "](/reference/$1$2)",
    )
    // Up-and-out links (../logo-assets/...) point at repository files with no
    // site route. Send them to the repository rather than to a dead path.
    .replace(
      /\]\(\.\.\/([^)]+)\)/g,
      "](https://github.com/EricWijesinghe/Maya2C/blob/master/$1)",
    );
}

const generated = [];

await rm(OUT, { recursive: true, force: true });
await mkdir(join(OUT, AUTHORED_OUT), { recursive: true });
await mkdir(join(OUT, REFERENCE_OUT), { recursive: true });

await writeFile(join(OUT, "index.mdx"), await readFile(HOME_SRC, "utf8"), "utf8");

// Authored pages first, verbatim: they already carry their own frontmatter and
// are written against the site's routes rather than GitHub's.
let authored = 0;
for (const name of (await readdir(AUTHORED_SRC)).sort()) {
  if (!name.endsWith(".md")) continue;
  await writeFile(
    join(OUT, AUTHORED_OUT, name),
    await readFile(join(AUTHORED_SRC, name), "utf8"),
    "utf8",
  );
  authored += 1;
}

for (const name of (await readdir(DOCS)).sort()) {
  if (!name.endsWith(".md") || SKIP.has(name)) continue;

  const raw = await readFile(join(DOCS, name), "utf8");
  const { title, body } = splitTitle(raw);

  if (title === null) {
    // Loud rather than skipped. A doc that silently never reaches the site is
    // the failure this whole script exists to avoid.
    throw new Error(
      `${name} has no '# H1'. Starlight needs a title and this script derives ` +
        `it from the H1 rather than inventing one from the filename.`,
    );
  }

  const slug = name.replace(/\.md$/, "");
  const frontmatter = [
    "---",
    `title: ${yamlString(title)}`,
    "editUrl: false",
    `# GENERATED from docs/${name} by scripts/ingest.mjs. Edit the source, not this.`,
    "---",
    "",
  ].join("\n");

  await writeFile(
    join(OUT, REFERENCE_OUT, name),
    frontmatter + rewriteLinks(body),
    "utf8",
  );
  generated.push({ slug, title });
}

// The sidebar is emitted rather than hand-listed in astro.config, so a new
// document in docs/ appears on the site without a second edit somewhere else.
await writeFile(
  join(here, "..", "src", "sidebar.generated.json"),
  JSON.stringify(
    generated.map(({ slug, title }) => ({
      label: title,
      link: `/${REFERENCE_OUT}/${slug}`,
    })),
    null,
    2,
  ) + "\n",
  "utf8",
);

console.log(
  `ingest: ${generated.length} from docs/, ${authored} authored from authored/`,
);
