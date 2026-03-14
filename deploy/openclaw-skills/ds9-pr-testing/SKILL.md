---
name: ds9-pr-testing
description: Validate DS9 code on a branch or PR against a realistic local stack and browser. Use when Sunil or a triage flow asks whether a DS9 fix actually works, or before claiming a branch/PR is tested. Bootstraps a PR checkout from a known-good DS9 checkout, copies working env/config, selects affected services, starts the minimum stack, verifies health, exercises the real route in a browser, inspects downloads/artifacts, and reports concrete evidence.
allowed-tools: Bash, Read, Write
---

# DS9 PR Testing

Use this skill whenever Linus needs to test DS9 code for real instead of stopping at code review.

If you have not brought up the local stack, opened the route in a browser, and inspected the resulting artifacts, do **not** say the branch or PR was tested. Say it was reviewed or prepared locally.

## Canonical variables

```bash
export SOURCE_DS9=/path/to/working/ds9
export TARGET_DS9=/path/to/ds9-pr-under-test
export ARTIFACT_DIR=/tmp/ds9-pr-test-artifacts
mkdir -p "$ARTIFACT_DIR"
```

- `SOURCE_DS9` is the known-good checkout with working secrets and local config.
- `TARGET_DS9` is the branch or PR checkout under test.
- If `TARGET_DS9` does not exist yet, create it as a worktree from `SOURCE_DS9`.

Known-good paths on Sunil's machines:
- macOS source checkout: `/Users/sunilrao/dev/ds9`
- `raoDesktop` WSL source checkout: `/home/sunil/ds9`

When in doubt, prefer the known-good checkout on the same machine over inventing fresh local config.

## First move: bootstrap from the working checkout

Never hand-author DS9 env files if a known-good local checkout already exists.

Helper locations:
- OpenClaw workspace copy: `/root/.openclaw/workspace/skills/ds9-pr-testing/scripts/`
- DS9 workstation copy: `/home/sunil/.local/share/linus/ds9-pr-testing/`

Use the workstation copy when the DS9 checkout lives on the remote development machine:

```bash
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/bootstrap_local_stack.sh \
  "$SOURCE_DS9" \
  "$TARGET_DS9" \
  "<branch-or-pr-ref>"
```

This will:
- create the target worktree if needed
- recursively copy real `.env`, `.env.*`, and `local.settings*.json` files from the working checkout
- skip `node_modules`, `.git`, `.claude`, and `*.sample` files
- print the declared Node versions for root, `lcars`, and `Q`

At minimum, these usually matter:
- `/.env`
- `/Q/.env`
- `/lcars/.env`
- `/lcars/.env.local`
- `/tribble-chat/.env`
- `/positronic-files/.env`
- `/positronic-files/local.settings.json`
- other `positronic-*/local.settings*.json` files touched by the change
- `/scripts/.env`

## Decide what to run

Start from the diff:

```bash
cd "$TARGET_DS9"
git diff --name-only origin/main...HEAD
```

Service map:
- `Q/*` -> run `Q`
- `lcars/src/*` or `lcars/ui/*` -> run `lcars`
- `apps/exocomp/*` -> run `apps/exocomp`
- `tribble-chat/*` -> run `tribble-chat`
- `positronic-files/*` -> run `positronic-files`
- `positronic-*/*` -> run the touched Azure Function
- `packages/*` -> rebuild and restart every consumer of the changed package

For most browser-based validation, the realistic minimum stack is:
1. `Q`
2. `lcars`
3. `apps/exocomp`
4. `positronic-files`
5. `tribble-chat`

## Node and prerequisites

Check the repo-declared Node versions:

```bash
cat "$TARGET_DS9/.node-version"
cat "$TARGET_DS9/lcars/.node-version"
cat "$TARGET_DS9/Q/.node-version"
```

In practice:
- root and `lcars` usually want Node `20.19.0`
- `Q` may want `18.16.1`
- `positronic-files` is safer on Node `20` than `24`

Machine prerequisites that must already work:
- `az`
- `func` (Azure Functions Core Tools v4)
- PostgreSQL with pgvector
- Python `3.10+`
- Playwright Chromium
- native libs for PDF / Office / image handling when export flows are involved

## Install and migrate

```bash
cd "$TARGET_DS9"
nvm use "$(cat .node-version)"
npm install
npm --prefix lcars/ui install
PLAYWRIGHT_BROWSERS_PATH=./ npx playwright install chromium
```

If the PR touches database code or adds migrations:

```bash
cd "$TARGET_DS9/lcars"
nvm use "$(cat ../.node-version)"
npm run execute-migration
```

## Minimal local DS9 data shape

Do not assume “local Postgres is up” means DS9 is ready. For DS9 local PR testing, the minimum useful DB shape is:
- `public` schema present with required extensions
- `tribble` schema present
- at least one real client schema like `c000001` with `client_setting`
- `tribble.llm_resource` populated with non-null encrypted `api_key` values so `Q` can decrypt and boot LLM resources

Unrelated warnings about missing tables in other client schemas can be acceptable. The DS9 README explicitly says warnings like `relation "c000034.client_setting" does not exist` are tolerable as long as the active development client imported correctly.

Before chasing app bugs, verify the local DB substrate:

```bash
DATABASE_URL="${DATABASE_URL:-postgres://tribbledev@localhost:5432/postgres}" \
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/verify_local_ds9_db.sh
```

If this check fails, do not claim the local stack is ready. Fix the DB first.

## Network access to Azure-backed local services

After syncing the real local config into the workstation checkout, DS9 local dev may still need Azure firewall/network rules opened for the workstation IP.

From the repo root on the workstation:

```bash
cd "$TARGET_DS9"
bash scripts/setupNetworkRulesDev.sh
```

Before relying on that script, verify:
- `az account show` works
- the Azure login is still fresh for a `@tribble.ai` account

If Azure says the refresh token expired, re-run:

```bash
az login --scope https://management.core.windows.net//.default
```

If `setupNetworkRulesDev.sh` fails while parsing `.env` or hits stale Azure CLI behavior, use the safe fallback helper from the workstation copy:

```bash
cd "$TARGET_DS9"
python3 /home/sunil/.local/share/linus/ds9-pr-testing/scripts/setup_network_rules_dev_safe.py
```

This helper:
- reads only the specific env keys it needs
- opens the test Postgres, Key Vault, and Airbyte paths
- avoids the brittle `export $(cat .env | xargs)` pattern
- avoids the stale Cognitive Services API path in the repo script

If both the repo script and the fallback helper fail, do not claim network setup succeeded. Report the exact blocker.

## Start the stack

Use separate PTYs or terminals so logs stay readable.

Before starting the UI, make local chat deterministic:

```bash
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/prepare_lcars_local_chat_env.sh "$TARGET_DS9"
```

Why this matters:
- `tribble-chat` listens on `3001`
- DS9 local `lcars/ui/.env` may still point `VITE_WEBCHAT_API_URL` at `ws://localhost:8080/api/chat`
- that stale value leaves the blue “Chat with Tribble” panel stuck on `Connecting...`
- old `VITE_LOCAL_DEV_AUTH_*` values in `.env.local` are also stale; DS9 does not implement that bypass, so remove them instead of treating them as a real auth lane

`prepare_lcars_local_chat_env.sh` writes an override in `lcars/ui/.env.local` so the browser uses:
- `VITE_WEBCHAT_API_URL=ws://localhost:3001/api/chat`
- `VITE_WEBCHAT_DOMAIN=localhost:3001`
- and strips old `VITE_LOCAL_DEV_AUTH_*` lines that would otherwise send Linus down the wrong path

Restart the UI after writing that override.

```bash
cd "$TARGET_DS9/Q" && nvm use "$(cat .node-version)" && npm run dev
cd "$TARGET_DS9/apps/exocomp" && nvm use 20.19.0 && PORT=3091 npm run dev
cd "$TARGET_DS9/tribble-chat" && nvm use 20.19.0 && npm run dev
cd "$TARGET_DS9/positronic-files" && nvm use 20.19.0 && npm run dev
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/start_lcars_canonical_local.sh "$TARGET_DS9" server
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/start_lcars_canonical_local.sh "$TARGET_DS9" ui
```

Expected ports:
- `50051` -> `Q`
- `3000` -> `lcars`
- `3091` -> `apps/exocomp`
- `7072` -> `positronic-files`
- `3001` -> `tribble-chat`
- `5173` -> `lcars/ui`

Do not treat `5174` as acceptable for authenticated DS9 testing. The frontend uses Auth0 with:

```tsx
redirect_uri: window.location.origin
```

That means the browser origin must be an Auth0-allowed callback origin. For local PR testing, use the canonical UI origin:

```text
http://localhost:5173
```

Do not navigate the browser to:
- `http://127.0.0.1:5173`
- `http://localhost:5174`
- `http://<wsl-host-ip>:5173`

Those origins may trigger Auth0 callback mismatch failures even when the Chrome profile is already logged in.

## Verify before opening the browser

Use the bundled verifier:

```bash
REQUIRED_PORTS="50051 3000 3091 7072 3001" \
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/verify_stack.sh
```

That checks listeners, `positronic-files` health, and the canonical UI origin.

If you have a real valid bearer token from the authenticated browser session, you may also pass:

```bash
AUTH_TOKEN="<real bearer token>" bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/verify_stack.sh
```

Do not fabricate a `local-dev-token` unless the branch explicitly implements a real local auth bypass.

Do not assume a local auth bypass exists. As of this skill version, DS9 does **not** implement the previously-documented `VITE_LOCAL_DEV_AUTH_BYPASS` / `LOCAL_DEV_AUTH_ENABLED` flow.

Default authenticated testing mode is:
- real Auth0 flow
- real logged-in Windows Chrome profile
- CDP attach from WSL
- canonical local UI origin `http://localhost:5173`

## Authenticated browser path via Windows Chrome CDP

When a real logged-in browser session is needed on `raoDesktop`, do not depend on an isolated Playwright profile. Use the dedicated Windows Chrome profile and bridge it into WSL.

From the workstation copy:

```bash
bash /home/sunil/.local/share/linus/ds9-pr-testing/scripts/ensure_windows_chrome_cdp_bridge.sh
```

This helper will:
- resolve the correct Windows profile directory dynamically
- start the dedicated Windows Chrome profile with CDP enabled if it is not already running
- start or restart a Windows-side TCP bridge on `9223` if needed
- print a working CDP endpoint that WSL can reach

The resolved endpoint is also written to:

```bash
/tmp/linus_chrome_cdp_endpoint.txt
```

Before claiming browser automation is ready, verify the endpoint is live:

```bash
curl -s "$(cat /tmp/linus_chrome_cdp_endpoint.txt)/json/version"
```

Use that resolved endpoint for Playwright `connectOverCDP` or any browser-tool connection. Do not hard-code `127.0.0.1:9222` from WSL; that is brittle in this environment.

For authenticated DS9 testing with CDP:
- navigate the browser to `http://localhost:5173`
- let Chrome reuse the existing Auth0 session if present
- if the app still redirects to Auth0 and fails callback, report an Auth0 origin mismatch instead of claiming the PR was tested
- if the browser is already on `http://localhost:5173/` with title `Tribble.ai`, treat auth as working and stop blaming Auth0 for downstream chat/socket issues

## Local chat and websocket truth

For local UI chat testing, the browser path is:
- UI at `http://localhost:5173`
- `tribble-chat` websocket backend at `ws://localhost:3001/api/chat`

The UI may still boot with a stale `VITE_WEBCHAT_API_URL=ws://localhost:8080/api/chat` from the committed `.env`. That is wrong for the current local DS9 topology. Override it locally before claiming chat is broken.

If the “Chat with Tribble” panel opens but stays on `Connecting...`, check these in order:
1. `tribble-chat` is actually listening on `3001`
2. `lcars/ui/.env.local` overrides `VITE_WEBCHAT_API_URL` to `ws://localhost:3001/api/chat`
3. the UI was restarted after the override
4. the browser console/network tab shows the websocket target you expect
5. the active local client schema has the settings tables the chat flow reads

Do not confuse a websocket/config problem with an Auth0 problem after the browser is already authenticated.

## Verbatim-document E2E truth

For issues like the verbatim document retrieval bug, “fully tested locally” means all of this happened:
1. local stack is running
2. authenticated local UI works in the CDP browser
3. a realistic synthetic document was created or uploaded
4. the document reached `Complete`
5. the document was tagged with a real verbatim metadata filter
6. the tag is visible on the source in the UI
7. the local chat flow answered the target question through the UI
8. you captured screenshots of both the tagged source and the successful answer

Useful UI landmarks:
- create/manage verbatim tags at `Settings -> Manage Tags` under the `BRAIN` section
- the local chat entry point is the blue “Chat with Tribble” critter in the top-left area of the UI

If you only validated:
- compile/typecheck
- unit tests
- direct DB rows
- embeddings or metadata existing in SQL

then the PR was **not** fully E2E tested yet. Say `build passed`, `typecheck passed`, `backend validated locally`, or `UI setup validated`, but do not say `tested` without the real chat/user flow.

If the local environment does not contain the original customer documents, create a realistic synthetic document and prove the exact behavior against that mock. Be explicit that the original customer data was not reproduced locally.

## Testing order

Always use this sequence:
1. Build or type-check the touched services.
2. Start only the needed services plus dependencies.
3. Manually smoke the affected route or flow.
4. Automate the exact user flow with Playwright or the browser tool.
5. Capture screenshots, downloads, browser console errors, page exceptions, and relevant service logs.
6. Inspect exported files or downloads.
7. Compare UI output to API or DB state when persistence matters.

## What to check in the browser

For any PR:
1. the route loads without auth loops
2. the relevant UI renders
3. no red console errors
4. no broken websocket or network spam
5. scroll and interactions work on real content
6. loading states terminate
7. buttons, drawers, editors, and modals actually work
8. data on screen matches the API response
9. downloads are real files
10. exported artifacts are opened and inspected

## Exports and generated files

Do not stop at “download happened.” Open the files.

```bash
mkdir -p "$ARTIFACT_DIR/unpacked"
unzip "/path/to/download.zip" -d "$ARTIFACT_DIR/unpacked"
find "$ARTIFACT_DIR/unpacked" -maxdepth 3 -type f | sort
```

Useful helpers for document inspection:

```bash
soffice --headless --convert-to pdf --outdir "$ARTIFACT_DIR" "/path/to/file.docx"
pdftoppm -png "/path/to/file.pdf" "$ARTIFACT_DIR/page"
pdftotext -layout "/path/to/file.pdf" "$ARTIFACT_DIR/file.txt"
```

Look for:
- placeholder text
- malformed tables
- missing sections
- empty manifests
- wrong filenames
- duplicate content

## Cleanup

Prefer app delete APIs over raw DB deletes when resetting state.

If local listeners are stale, kill them explicitly and restart the stack:

```bash
lsof -ti tcp:3000 | xargs kill -9 2>/dev/null || true
lsof -ti tcp:7072 | xargs kill -9 2>/dev/null || true
lsof -ti tcp:3091 | xargs kill -9 2>/dev/null || true
lsof -ti tcp:50051 | xargs kill -9 2>/dev/null || true
lsof -ti tcp:8080 | xargs kill -9 2>/dev/null || true
lsof -ti tcp:5173 | xargs kill -9 2>/dev/null || true
lsof -ti tcp:5174 | xargs kill -9 2>/dev/null || true
```

## Reporting

A useful test report must say:
- which branch or PR was tested
- which services were actually running
- whether auth bypass or real login was used
- which route or flow was exercised
- what screenshots, downloads, or logs were captured
- whether exported files were opened and what they contained
- exact failures, not generic “didn’t work”

In shared Slack channels, keep the report group-safe and outcome-focused. In private operator contexts with Sunil, include concrete commands, paths, and evidence.

In shared Slack channels, do not stream console thoughts line-by-line. Use only:
- one short acknowledgement when work starts, if helpful
- one blocker update if genuinely stuck
- one final evidence-backed summary

Use precise status labels:
- `reviewed`
- `build passed`
- `typecheck passed`
- `backend validated locally`
- `UI auth validated`
- `fully locally tested`
- `staging-tested`

## DS9 triage integration

If a DS9 triage flow prepares code on a branch, use this skill before saying:
- “reviewed and tested”
- “ready to merge”
- “confirmed fixed”

If you only reviewed the diff or ran unit tests, say that plainly.
