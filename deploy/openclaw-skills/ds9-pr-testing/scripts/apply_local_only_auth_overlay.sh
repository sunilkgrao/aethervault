#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 TARGET_DS9 [EMAIL]" >&2
  exit 1
fi

TARGET_DS9="$1"
EMAIL="${2:-sunil@tribble.ai}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE_PATH="$(cd "$SCRIPT_DIR/.." && pwd)/templates/localAuth0.tsx"
BACKUP_ROOT="$TARGET_DS9/.linus-local-only-auth-backup"

required_files=(
  "$TARGET_DS9/lcars/ui/vite.config.ts"
  "$TARGET_DS9/lcars/src/utils/auth.ts"
  "$TARGET_DS9/tribble-chat/src/utils/auth.ts"
  "$TARGET_DS9/lcars/src/app.ts"
  "$TARGET_DS9/tribble-chat/src/app.ts"
)

for path in "${required_files[@]}"; do
  if [[ ! -f "$path" ]]; then
    echo "missing required file: $path" >&2
    exit 1
  fi
done

if [[ ! -f "$TEMPLATE_PATH" ]]; then
  echo "missing template: $TEMPLATE_PATH" >&2
  exit 1
fi

backup_once() {
  local path="$1"
  local rel="${path#"$TARGET_DS9"/}"
  local dest="$BACKUP_ROOT/$rel"
  if [[ ! -e "$dest" ]]; then
    mkdir -p "$(dirname "$dest")"
    cp "$path" "$dest"
  fi
}

for path in "${required_files[@]}"; do
  backup_once "$path"
done

LOCAL_AUTH0_PATH="$TARGET_DS9/lcars/ui/src/dev/localAuth0.tsx"
mkdir -p "$(dirname "$LOCAL_AUTH0_PATH")"
if [[ ! -e "$BACKUP_ROOT/lcars/ui/src/dev/localAuth0.tsx" && -f "$LOCAL_AUTH0_PATH" ]]; then
  mkdir -p "$BACKUP_ROOT/lcars/ui/src/dev"
  cp "$LOCAL_AUTH0_PATH" "$BACKUP_ROOT/lcars/ui/src/dev/localAuth0.tsx"
fi
cp "$TEMPLATE_PATH" "$LOCAL_AUTH0_PATH"

python3 - "$TARGET_DS9" <<'PY'
from pathlib import Path
import sys

target = Path(sys.argv[1])

def patch_dotenv(path: Path):
    marker = "require('dotenv').config({ path: '.env.local', override: true });"
    text = path.read_text()
    if marker in text:
        return
    original = "require('dotenv').config();\n"
    replacement = "require('dotenv').config();\nrequire('dotenv').config({ path: '.env.local', override: true });\n"
    if original not in text:
        raise SystemExit(f"dotenv bootstrap not found in {path}")
    path.write_text(text.replace(original, replacement, 1))

def patch_vite(path: Path):
    text = path.read_text()
    if "const authBypass = env.VITE_LOCAL_DEV_AUTH_BYPASS === 'true';" in text:
        return
    text = text.replace(
        "import { defineConfig, splitVendorChunkPlugin } from 'vite';",
        "import { defineConfig, loadEnv, splitVendorChunkPlugin } from 'vite';",
        1,
    )
    text = text.replace(
        "export default defineConfig(({ command, mode, ssrBuild }) => {\n",
        "export default defineConfig(({ command, mode, ssrBuild }) => {\n  const env = loadEnv(mode, process.cwd(), '');\n  const apiOrigin = env.VITE_LOCAL_API_ORIGIN || 'http://localhost:3000';\n  const authBypass = env.VITE_LOCAL_DEV_AUTH_BYPASS === 'true';\n",
        1,
    )
    alias_target = "          exceljs: 'exceljs/dist/exceljs.bare.min.js',\n"
    alias_replacement = "          exceljs: 'exceljs/dist/exceljs.bare.min.js',\n          ...(authBypass ? { '@auth0/auth0-react': path.resolve(__dirname, './src/dev/localAuth0.tsx') } : {}),\n"
    if text.count(alias_target) < 2:
        raise SystemExit(f"unexpected alias blocks in {path}")
    text = text.replace(alias_target, alias_replacement)
    text = text.replace("'/api': 'http://localhost:3000',", "'/api': apiOrigin,", 1)
    text = text.replace("'/auth': 'http://localhost:3000',", "'/auth': apiOrigin,", 1)
    path.write_text(text)

AUTH_HELPERS = """
function resolveLocalDevAuthEmail(rawToken?: string): string | undefined {
  if (process.env.LOCAL_DEV_AUTH_ENABLED !== 'true' || !rawToken) {
    return undefined;
  }

  if (
    process.env.LOCAL_DEV_AUTH_TOKEN &&
    process.env.LOCAL_DEV_AUTH_EMAIL &&
    rawToken === process.env.LOCAL_DEV_AUTH_TOKEN
  ) {
    return process.env.LOCAL_DEV_AUTH_EMAIL.toLowerCase();
  }

  if (rawToken.startsWith('local-dev-token:')) {
    return rawToken.slice('local-dev-token:'.length).toLowerCase();
  }

  if (rawToken.startsWith('local-dev-token.')) {
    return rawToken
      .slice('local-dev-token.'.length)
      .replace(/__at__/g, '@')
      .toLowerCase();
  }

  return undefined;
}

function tryAttachLocalDevAuth(req: any): boolean {
  const bearerToken = req.headers?.authorization;
  const rawToken =
    typeof bearerToken === 'string' && bearerToken.startsWith('Bearer ')
      ? bearerToken.split(' ')[1]
      : undefined;
  const email = resolveLocalDevAuthEmail(rawToken);
  if (!email) {
    return false;
  }

  const namespace = process.env.AUTH0_API_AUDIENCE || '';
  req.auth = {
    payload: {
      email,
      sub: `local-dev|${email}`,
      [`${namespace}email`]: email,
    },
  };
  return true;
}

""".lstrip()

JWT_BLOCK = """const auth0JwtCheck = auth({
  audience: process.env.AUTH0_API_AUDIENCE,
  issuerBaseURL: process.env.AUTH0_API_ISSUER_BASE_URL,
  tokenSigningAlg: process.env.AUTH0_API_TOKEN_SIGNING_ALGO,
});

module.exports.jwtCheck = (req: any, res: Response, next: NextFunction) => {
  if (tryAttachLocalDevAuth(req)) {
    return next();
  }
  return auth0JwtCheck(req, res, next);
};
"""

def patch_auth(path: Path):
    text = path.read_text()
    if "function resolveLocalDevAuthEmail(rawToken?: string): string | undefined {" not in text:
        anchor = "const m2mCheck = async (req: any, res: Response, next: NextFunction) => {\n"
        if anchor not in text:
            raise SystemExit(f"m2mCheck anchor not found in {path}")
        text = text.replace(anchor, AUTH_HELPERS + anchor, 1)
    old = """module.exports.jwtCheck = auth({
  audience: process.env.AUTH0_API_AUDIENCE,
  issuerBaseURL: process.env.AUTH0_API_ISSUER_BASE_URL,
  tokenSigningAlg: process.env.AUTH0_API_TOKEN_SIGNING_ALGO,
});
"""
    if "const auth0JwtCheck = auth({" not in text:
        if old not in text:
            raise SystemExit(f"jwtCheck block not found in {path}")
        text = text.replace(old, JWT_BLOCK, 1)
    path.write_text(text)

patch_dotenv(target / "lcars/src/app.ts")
patch_dotenv(target / "tribble-chat/src/app.ts")
patch_vite(target / "lcars/ui/vite.config.ts")
patch_auth(target / "lcars/src/utils/auth.ts")
patch_auth(target / "tribble-chat/src/utils/auth.ts")
PY

cat <<EOF
applied local-only auth overlay to $TARGET_DS9
email=${EMAIL}
backup_root=${BACKUP_ROOT}
local_auth0=${LOCAL_AUTH0_PATH}

warning: this overlay is for local droplet/workstation testing only.
Do not commit or push these DS9 worktree edits.
Run revert_local_only_auth_overlay.sh before preparing a PR branch.
EOF
