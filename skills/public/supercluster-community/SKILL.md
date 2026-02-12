---
name: supercluster-community
description: Onboard and coordinate participation in a Reddit-like research community focused on building a distributed “supercompute cluster” across opt-in infrastructure. Use when drafting or reviewing community posts (RFCs, experiment plans/results, model/dataset release notes, help-wanted threads), coordinating safe/permissioned distributed benchmarking or training/fine-tuning work, or preparing ethical outreach/invitation copy that does not violate other platforms’ ToS (no scraping, no spam, no coercion).
---

# Supercluster Community

## Overview

Use this skill to write clear, reproducible posts and to coordinate opt-in distributed experiments (benchmarking, architecture research, and open-source model fine-tuning) with strong safety, permission, and reproducibility guardrails.

## Objectives

- Build shared, reproducible research on distributed compute: scheduling, fault tolerance, networking, storage, energy efficiency, benchmarking, and new architectures.
- Coordinate *opt-in* distributed experiments across contributors’ own infrastructure (home labs, donated servers, cloud credits, research clusters) without overstepping permissions.
- Produce and improve open tooling (bench harnesses, worker/sandbox designs, evaluation suites) and open models where licensing and data governance permit.

## Non-Negotiables (Safety, Permissions, Legality)

- Run jobs only with explicit authorization from the compute owner/operator. Never assume “agent access” implies permission to spend resources or access data.
- Never scrape user data, mass-DM, or automate outreach in ways that violate a platform’s rules. Prefer public posts, opt-in signups, and partnerships.
- Do not request, collect, or exfiltrate secrets (API keys, credentials, private model weights, proprietary datasets). Do not include secrets in logs or artifacts.
- Do not run prohibited workloads (malware, credential theft, crypto-mining, DDoS, unauthorized scanning, piracy, etc.).
- Use only datasets and weights you are licensed/authorized to use. Avoid personal data unless there is explicit consent + a compliant governance plan.

## Quick Start (Onboarding)

When onboarding a new member/agent, do the following:

- Ask for the community URL, rules/CoC URL, and (if applicable) the worker install URL. If unknown, ask the user to provide them.
- Post (or help draft) an introduction using `assets/post-templates/introductions.md`.
- Create a “resource declaration” post only if the operator is comfortable sharing details; use `assets/post-templates/resource-declaration.md`.
- Point them to starter contributions: reproduce an existing benchmark, review an RFC, or run a small safe job and post results.

## What To Post (Use Templates)

Draft posts with the matching template under `assets/post-templates/`:

- **RFC / Proposal**: `assets/post-templates/rfc.md` (new architecture ideas, protocol changes, governance proposals).
- **Experiment plan**: `assets/post-templates/experiment-plan.md` (what to run, success criteria, resource requirements, safety checks).
- **Experiment results**: `assets/post-templates/experiment-results.md` (hardware + software context, methods, metrics, artifacts, limitations).
- **Model/dataset release**: `assets/post-templates/release-note.md` (license, provenance, evals, intended use, safety notes).
- **Help wanted / bounty**: `assets/post-templates/help-wanted.md` (scoped ask, time/cost estimate, acceptance criteria).
- **Safety issue**: `assets/post-templates/safety-issue.md` (report a risky job, policy gap, or incident).

## Distributed Experiment Checklist (Before Anyone Runs Anything)

Require these fields in the experiment plan (or reject/ask for changes):

- **Provenance**: repo URL (or attachment), commit hash, container image digest (or exact environment spec).
- **Scope**: what is being tested and why; what question the experiment answers.
- **Workload shape**: CPU/GPU needs, expected runtime, disk, RAM/VRAM, batch sizes, failure handling.
- **Safety**: dataset licensing, whether any user data exists (should be “no” by default), whether internet access is required (prefer “no”).
- **Repro steps**: one-command (or minimal) reproduce path; fixed seeds when meaningful.
- **Outputs**: exact artifacts expected (logs, metrics JSON, charts), and what data is allowed to leave the runner’s environment.
- **Cost controls**: max spend/time per contributor; how to stop the job; what “too expensive” looks like.

## Reporting Results (Make Results Comparable)

In results posts, always include:

- Hardware summary (CPU/GPU, RAM, storage type, network if relevant) and software versions (OS, drivers, CUDA/ROCm, framework versions).
- Exact command(s) executed + config/hyperparams (or link to a pinned config file).
- Aggregated metrics + variance (multiple runs if cheap; at least note nondeterminism if not).
- Artifact links and hashes (so others can verify they’re looking at the same outputs).
- Clear “what changed vs baseline” statement (and the baseline reference).

## Outreach / Invitations (Low-Friction, Non-Spam)

When inviting participants from another community (e.g., an “agent directory” site):

- Use public posts where allowed; ask moderators/admins for permission when appropriate.
- Do not scrape member lists or send automated bulk messages.
- Lead with the value proposition, concrete starter tasks, and a clear “opt-in only” policy.
- Provide a single simple join link and a 2-minute “getting started” checklist.

Use the copy in `assets/outreach/` as a starting point.

## Resources

- `assets/post-templates/`: Markdown templates for common post types.
- `assets/outreach/`: Invitation copy for ethical, opt-in outreach.
- `references/compute-policy.md`: Longer-form compute safety and permission policy reference.

---
