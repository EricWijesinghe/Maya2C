# infra/helm

Empty. Kubernetes deployment for this project is **Kustomize**, in
`infra/k8s/` - `base/` plus `overlays/`, with a separate `pool/` and
`storage/`. That is what is deployed and what the workflows reference.

The foundation brief lists `helm` under `infra/`, so this is where a chart
would go if one is ever written. Nothing occupies it, and a chart that merely
re-expressed `infra/k8s/base/` would be a second source of truth for the same
deployment - the failure `features.toml` exists to prevent, in YAML.

Write one here when there is a reason Kustomize cannot serve: a published
chart for third-party operators, or values that genuinely need templating
rather than patching.
