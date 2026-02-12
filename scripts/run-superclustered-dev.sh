#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ ! -d ".venv" ]]; then
  python3 -m venv .venv
fi

# shellcheck disable=SC1091
. .venv/bin/activate

pip install -r requirements.txt >/dev/null

export DJANGO_DEBUG="${DJANGO_DEBUG:-1}"
export DJANGO_SECRET_KEY="${DJANGO_SECRET_KEY:-dev-secret}"

PORT="${PORT:-8000}"

cd superclustered
python manage.py migrate >/dev/null

echo "Starting dev server: http://127.0.0.1:${PORT}/"
python manage.py runserver "127.0.0.1:${PORT}"

