#!/usr/bin/env bash
set -euo pipefail

DATABASE_URL="${DATABASE_URL:-postgres://tribbledev@localhost:5432/postgres}"

run_sql() {
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc "$1"
}

if ! run_sql "select 1" >/dev/null 2>&1; then
  echo "cannot connect to local Postgres with DATABASE_URL=$DATABASE_URL" >&2
  exit 1
fi

missing=0

tribble_tables="$(run_sql "select count(*) from information_schema.tables where table_schema = 'tribble'")"
client_schema="$(run_sql "select table_schema from information_schema.tables where table_name = 'client_setting' and table_schema ~ '^c[0-9]{6}$' order by table_schema limit 1")"
client_tables=""
if [[ -n "$client_schema" ]]; then
  client_tables="$(run_sql "select count(*) from information_schema.tables where table_schema = '$client_schema'")"
fi
llm_resource_present="$(run_sql "select count(*) from information_schema.tables where table_schema = 'tribble' and table_name = 'llm_resource'")"
llm_resources="0"
llm_keys="0"
if [[ "$llm_resource_present" != "0" ]]; then
  llm_resources="$(run_sql "select count(*) from tribble.llm_resource")"
  llm_keys="$(run_sql "select count(*) from tribble.llm_resource where api_key is not null and api_key <> ''")"
fi

printf 'database_url=%s\n' "$DATABASE_URL"
printf 'tribble_tables=%s\n' "$tribble_tables"
printf 'active_client_schema=%s\n' "${client_schema:-missing}"
printf 'active_client_tables=%s\n' "${client_tables:-0}"
printf 'llm_resources=%s\n' "$llm_resources"
printf 'llm_resources_with_keys=%s\n' "$llm_keys"

if [[ "$tribble_tables" -eq 0 ]]; then
  echo "missing tribble schema tables" >&2
  missing=1
fi

if [[ -z "$client_schema" ]]; then
  echo "missing client schema with client_setting table" >&2
  missing=1
fi

if [[ "$llm_resource_present" -eq 0 ]]; then
  echo "missing tribble.llm_resource table" >&2
  missing=1
fi

if [[ "$llm_keys" -eq 0 ]]; then
  echo "tribble.llm_resource has no non-null api_key values; Q will not boot cleanly" >&2
  missing=1
fi

exit "$missing"
