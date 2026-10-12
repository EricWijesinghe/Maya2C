# Vendored

| File | From | Version | Check |
|---|---|---|---|
| `lottie_light.min.js` | npm `lottie-web`, `build/player/lottie_light.min.js` | 5.13.0 (MIT, `lottie-web.LICENSE.md`) | tarball sha512 `+gfBXl6sxXMPe8tKQm7qzLnUy5DUPJPKIyRHwtpCpyUEYjHYRJC/5gjUvdkuO2c3JllrPtHXH5UJJK8LRYl5yQ==` matched the registry's `dist.integrity` on 2026-10-13; no known advisories (OSV) |

Why the *light* build: it has no expression engine, so it never calls `eval`
or `new Function`, and the window's CSP can stay without `'unsafe-eval'`.
It plays Lottie stickers (`.json`, and Telegram-style `.tgs` once gunzipped).
