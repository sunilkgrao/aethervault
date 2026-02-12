# Experiment Plan Template

**Title:** Experiment plan: `<what you will test>` (baseline vs. variant)

## Goal
<!-- The specific question this experiment answers -->

## Hypothesis
<!-- What you expect to happen and why -->

## Workload summary
- Type: <!-- benchmark / simulation / training / finetune / eval -->
- Expected runtime: <!-- per run -->
- Resource needs: <!-- CPU/GPU/RAM/VRAM/disk -->
- Internet required: <!-- no (default) / yes (justify) -->

## Provenance (required)
- Repo / code: <!-- link -->
- Commit hash: <!-- SHA -->
- Environment: <!-- container image digest or exact setup -->
- Config: <!-- link/file -->
- Dataset(s): <!-- name + version + license -->

## Method
- Baseline:
- Variant(s):
- Metrics:
- Number of runs / seeds:

## Safety + permission checklist (required)
- Authorization to run workloads: <!-- yes/no + notes -->
- Dataset licensing verified: <!-- yes/no -->
- Contains personal data: <!-- no (default) / yes (must explain governance) -->
- Contains secrets/credentials: <!-- no -->
- Network egress needed: <!-- no (default) / yes (must explain destinations) -->

## Outputs
- Expected artifacts: <!-- logs/metrics JSON/charts -->
- Allowed-to-share outputs only: <!-- confirm -->

## Cost controls
- Max time per contributor:
- Max spend (if any):
- Stop conditions:

## How to reproduce
```bash
# Provide a minimal command sequence that others can run
```

## Who can help / needed resources
- <!-- what hardware profiles are needed -->
