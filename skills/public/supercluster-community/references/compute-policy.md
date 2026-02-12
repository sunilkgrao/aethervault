# Compute Policy (Reference)

Use this as a reference when drafting rules, reviewing experiment plans, or designing the distributed worker/scheduler.

## Consent and authorization
- Run jobs only when the runner/operator explicitly opted in.
- Assume “no permission” by default for any resource, dataset, or credential.
- Prefer per-job approval for any job that is new, long-running, costly, or requests network access.

## Default technical safety constraints
- Sandbox jobs (container/VM/WASM). Do not run arbitrary host commands.
- Enforce resource limits: CPU, RAM, disk, GPU time/VRAM where possible.
- Default to **no network egress**. If allowed, use an allowlist of destinations and protocols.
- Treat all job outputs as public; forbid secrets in outputs.
- Require pinned environments (container digest) and a signed job manifest.

## Prohibited workloads
- Malware, credential theft, unauthorized scanning, exploitation, DDoS.
- Crypto-mining or covert monetization.
- Any workload that violates the runner’s policies, employer rules, cloud ToS, or local law.

## Data handling
- Prefer public datasets with clear licenses.
- Avoid personal data by default. If personal data is involved, require explicit governance, consent, minimization, and retention policy.
- Do not request uploading proprietary datasets to shared storage unless explicitly authorized.

## Reporting and incident response
- Provide a clear “report safety issue” workflow and a designated response team.
- Stop distribution quickly: revoke job keys/signing, block job IDs, and publish a postmortem when appropriate.
