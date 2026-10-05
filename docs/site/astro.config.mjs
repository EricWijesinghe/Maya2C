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
  site: "https://maya2c.dev",
  integrations: [
    starlight({
      title: "Maya2C",
      favicon: "/favicon.ico",
      // Bing Webmaster Tools ownership check for maya2c.dev.
      head: [
        { tag: "meta", attrs: { name: "msvalidate.01", content: "76B55D3E70EB56CB2D46FE438641E2A4" } },
        // Link previews (Slack, X, Discord, LinkedIn): Starlight emits the
        // title and description, but no image unless one is named here.
        { tag: "meta", attrs: { property: "og:image", content: "https://maya2c.dev/og-card.png" } },
        { tag: "meta", attrs: { property: "og:image:width", content: "1200" } },
        { tag: "meta", attrs: { property: "og:image:height", content: "630" } },
        { tag: "meta", attrs: { property: "og:image:alt", content: "Maya2C — post-quantum layer-1 blockchain" } },
        { tag: "meta", attrs: { name: "twitter:image", content: "https://maya2c.dev/og-card.png" } },
        { tag: "meta", attrs: { name: "theme-color", content: "#050b0f" } },
        // Structured data, so search engines know what the site is about:
        // the project, its source, and that the software is free.
        {
          tag: "script",
          attrs: { type: "application/ld+json" },
          content: JSON.stringify({
            "@context": "https://schema.org",
            "@graph": [
              {
                "@type": "WebSite",
                name: "Maya2C",
                url: "https://maya2c.dev/",
              },
              {
                "@type": "SoftwareApplication",
                name: "Maya2C node",
                applicationCategory: "DeveloperApplication",
                operatingSystem: "Linux, Windows, macOS",
                license: "https://www.apache.org/licenses/LICENSE-2.0",
                codeRepository: "https://github.com/EricWijesinghe/Maya2C",
                offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
                description:
                  "Post-quantum layer-1 blockchain node: DAG-BFT finality and hybrid ML-DSA + SLH-DSA signatures.",
              },
            ],
          }),
        },
      ],
      description:
        "Post-quantum layer-1 blockchain. DAG-BFT finality, hybrid ML-DSA + " +
        "SLH-DSA signatures, shielded transfers.",
      // Self-hosted variable fonts: no third-party request, no layout shift
      // from a late swap, and the same letterforms on every OS.
      customCss: [
        "@fontsource-variable/manrope",
        "@fontsource-variable/inter",
        "@fontsource-variable/jetbrains-mono",
        "./src/styles/maya.css",
      ],
      components: {
        // Our footer (about, contact, copyright) after Starlight's own.
        Footer: "./src/components/Footer.astro",
        // The real emblem and wordmark, and a header with main navigation,
        // reading progress and back-to-top.
        SiteTitle: "./src/components/SiteTitle.astro",
        Header: "./src/components/Header.astro",
      },
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/EricWijesinghe/Maya2C" },
      ],
      sidebar: [
        {
          label: "Guides",
          items: [
            { label: "Experience Maya2C", link: "/experience/" },
            { label: "Join the testnet", link: "/guides/testnet" },
            { label: "Quickstart", link: "/guides/quickstart" },
            { label: "Post-quantum signatures", link: "/guides/signatures" },
            { label: "Mining and validators", link: "/guides/mining" },
            { label: "Run a validator", link: "/guides/validators" },
            { label: "Break it", link: "/guides/break-it" },
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
