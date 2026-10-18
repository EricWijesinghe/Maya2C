// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import { readFileSync } from "node:fs";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import { unified } from "@astrojs/markdown-remark";

// Emitted by scripts/ingest.mjs, which runs before every build. Generated
// rather than hand-listed so a new file in docs/ reaches the site without a
// second edit here — the failure that produces is a document nobody can find,
// and nothing reports it.
const reference = JSON.parse(
  readFileSync(new URL("./src/sidebar.generated.json", import.meta.url), "utf8"),
);

export default defineConfig({
  site: "https://maya2c.dev",
  // Equations in the "For scientists" tabs. KaTeX renders at build time, so
  // the page ships HTML and no math script (the CSP allows none from outside).
  markdown: {
    processor: unified({
      remarkPlugins: [remarkMath],
      rehypePlugins: [rehypeKatex],
    }),
  },
  // Content-Security-Policy (incident 2026-10-06): a hijacked edge appended
  // a ClickFix loader to every page. Astro hashes every script and style it
  // emits, so a script added after the build has no hash and browsers refuse
  // to run it. A meta CSP cannot stop someone who controls the edge and
  // strips it; it stops everything else that injects into the page.
  security: {
    csp: {
      // Styles cannot run code. Inline style attributes (code highlighting,
      // the launch-status rail) need 'unsafe-inline'; scripts stay hash-only.
      styleDirective: {
        resources: ["'self'", "'unsafe-inline'"],
      },
      scriptDirective: {
        // Pagefind (search) runs WebAssembly from /pagefind/.
        resources: ["'self'", "'wasm-unsafe-eval'"],
        // Starlight's own is:inline scripts (theme provider and pickers,
        // search shortcut, sidebar restore and scroll), which Astro does not
        // hash. Reviewed 2026-10-06 for @astrojs/starlight 0.42.5;
        // scripts/check-csp.mjs fails the build if one changes.
        hashes: [
          "sha256-7eCV4jtsr4t4knb3c4FCRPeu7GGZeOUGE3XvWix0XOQ=",
          // Head.astro: sets data-section and data-motif before first paint. Reviewed 2026-10-18.
          "sha256-6ZINGwbBwZAoVmAasCd7cTtRBKBuDMifxtzCpxtsf9c=",
          "sha256-GkZBRnvSuhtx/cvzvukVkX2JJZW+DdPlVr7BX8Tefqo=",
          "sha256-VWo5Wp4aqSj6nSgMpeAp9cKieaoIfwFUAunAVugI5gA=",
          "sha256-f/zAUE74ucc3JYp4r4QQvkJofoQdkOIhHYK+jeZ6eko=",
          "sha256-wX2yOADeV+NMngflD5uYi3vl50SHC4sfM1EmylVjlX4=",
          // <Tabs syncKey>: restores the reader's chosen OS tab from
          // localStorage; reads one key, no network. Reviewed 2026-10-12.
          "sha256-OizSKqsU+f0G4vojbxNt0Lao3kUpTmCLQSv3y6P7qhQ=",
        ],
      },
      directives: [
        "default-src 'self'",
        // Only our own live services: RPC, explorer (and its block stream),
        // status, faucet.
        "connect-src 'self' https://rpc.maya2c.dev https://explorer.maya2c.dev wss://explorer.maya2c.dev https://status.maya2c.dev https://faucet.maya2c.dev",
        "img-src 'self' data: https://img.shields.io",
        "font-src 'self' data:",
        "object-src 'none'",
        "base-uri 'self'",
        "form-action 'self' https://faucet.maya2c.dev",
        "frame-src 'none'",
        "upgrade-insecure-requests",
      ],
    },
  },
  integrations: [
    starlight({
      title: "Maya2C",
      favicon: "/favicon.ico",
      // Bing Webmaster Tools ownership check for maya2c.dev.
      head: [
        { tag: "meta", attrs: { name: "msvalidate.01", content: "76B55D3E70EB56CB2D46FE438641E2A4" } },
        // Link-preview images are per section, set in src/components/Head.astro.
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
                "@type": "Organization",
                "@id": "https://maya2c.dev/#org",
                name: "Maya2C",
                url: "https://maya2c.dev/",
                logo: "https://maya2c.dev/apple-touch-icon.png",
                sameAs: [
                  "https://github.com/EricWijesinghe/Maya2C",
                  "https://www.linkedin.com/in/ericwijesinghe",
                  "https://www.facebook.com/EricWijesinghe2002/",
                  "https://www.instagram.com/ericwijesinghe2022/",
                  "https://eric.maya2c.dev",
                ],
              },
              {
                "@type": "WebSite",
                "@id": "https://maya2c.dev/#website",
                name: "Maya2C",
                url: "https://maya2c.dev/",
                inLanguage: "en",
                publisher: { "@id": "https://maya2c.dev/#org" },
              },
              {
                "@type": "SoftwareApplication",
                "@id": "https://maya2c.dev/#node",
                name: "Maya2C node",
                applicationCategory: "DeveloperApplication",
                operatingSystem: "Linux, Windows, macOS",
                softwareVersion: "0.1.0-testnet.2",
                downloadUrl: "https://github.com/EricWijesinghe/Maya2C/releases",
                license: "https://www.apache.org/licenses/LICENSE-2.0",
                publisher: { "@id": "https://maya2c.dev/#org" },
                offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
                description:
                  "Post-quantum layer-1 blockchain node: DAG-BFT finality and hybrid ML-DSA + SLH-DSA signatures.",
              },
              {
                "@type": "SoftwareSourceCode",
                name: "Maya2C",
                codeRepository: "https://github.com/EricWijesinghe/Maya2C",
                programmingLanguage: "Rust",
                license: "https://www.apache.org/licenses/LICENSE-2.0",
                publisher: { "@id": "https://maya2c.dev/#org" },
              },
            ],
          }),
        },
      ],
      description:
        // No "shielded transfers": mainnet v1 launches without them
        // (decided 2026-09-30), and a description must not promise them.
        "Post-quantum layer-1 blockchain in Rust: DAG-BFT finality and " +
        "hybrid ML-DSA + SLH-DSA signatures on every transaction.",
      // Self-hosted variable fonts: no third-party request, no layout shift
      // from a late swap, and the same letterforms on every OS.
      customCss: [
        "@fontsource-variable/manrope",
        "@fontsource-variable/inter",
        "@fontsource-variable/jetbrains-mono",
        // KaTeX glyph styles, self-hosted from node_modules (fonts are same-origin).
        "katex/dist/katex.min.css",
        "./src/components/math/katex-overflow.css",
        "./src/styles/maya.css",
        "./src/styles/maya-living.css",
      ],
      components: {
        // Our footer (about, contact, copyright) after Starlight's own.
        Footer: "./src/components/Footer.astro",
        // The real emblem and wordmark, and a header with main navigation,
        // reading progress and back-to-top.
        SiteTitle: "./src/components/SiteTitle.astro",
        Header: "./src/components/Header.astro",
        // No second h1 on the homepage, whose hero headline is the h1.
        PageTitle: "./src/components/PageTitle.astro",
        // Per-page structured data: breadcrumbs and a TechArticle.
        Head: "./src/components/Head.astro",
      },
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/EricWijesinghe/Maya2C" },
      ],
      sidebar: [
        {
          label: "Guides",
          items: [
            { label: "Experience Maya2C", link: "/experience/" },
            { label: "Help run the network (easy)", link: "/guides/help-run" },
            { label: "Join the testnet", link: "/guides/testnet" },
            { label: "Quickstart", link: "/guides/quickstart" },
            { label: "How a transaction works", link: "/guides/transactions" },
            { label: "Post-quantum signatures", link: "/guides/signatures" },
            { label: "Consensus and finality", link: "/guides/consensus" },
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
