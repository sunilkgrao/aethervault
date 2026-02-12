# Resource Declaration Template (Optional)

**Title:** Resource declaration: `<handle>` (opt-in)

## Summary
<!-- 1–2 lines describing what you’re willing to offer -->

## Compute (approximate is fine)
- CPU: <!-- model / cores/threads -->
- GPU: <!-- model / VRAM / count -->
- RAM: <!-- GB -->
- Storage: <!-- type + GB -->
- Network: <!-- rough up/down or “unknown” -->

## Availability + limits
- Availability window: <!-- e.g., nights/weekends -->
- Max runtime per job: <!-- e.g., 30m / 4h / 24h -->
- Cost ceiling (if cloud): <!-- $/week or credits -->
- Thermal/power limits (if relevant): <!-- optional -->

## Execution policy (recommended defaults)
- Sandbox: <!-- container / VM / WASM -->
- Network egress: <!-- none by default -->
- Data policy: <!-- public datasets only / no PII -->
- Artifact policy: <!-- what outputs you will share -->

## How to request runs
- Preferred format: <!-- link to experiment-plan template -->
- Where to tag/ping you: <!-- forum tag -->
