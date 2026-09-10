// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import { readFileSync } from "node:fs";

// Emitted by scripts/ingest.mjs, which runs before every build. Generated
// rather than hand-listed so a new file in docs/ reaches the site without a
// second edit here — the failure that produces is a document nobody can find,
// and nothing reports it.
const reference = JSON.parse(
  readFileSync(new URL("./src/sidebar.generated.json", import.meta.url), "utf8"),
);

export default defineConfig({
  site: "https://docs.maya2c.example",
  integrations: [
    starlight({
      title: "Maya2C",
      description:
        "Post-quantum layer-1 blockchain. Hybrid ML-DSA + SLH-DSA signatures, " +
        "proof of work, shielded transfers.",
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/maya2c/maya2c" },
      ],
      sidebar: [
        {
          label: "Guides",
          items: [
            { label: "API reference", link: "/guides/api" },
            { label: "Wallet integration", link: "/guides/wallet" },
            { label: "Compiling a contract", link: "/guides/contracts" },
          ],
        },
        // The whole of docs/, in the order ingest found it.
        { label: "Reference", items: reference },
      ],
    }),
  ],
});
