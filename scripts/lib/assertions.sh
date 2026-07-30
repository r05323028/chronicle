#!/usr/bin/env bash

assert_file() { [[ -f $1 ]] || die "missing file: $1"; }
assert_dir() { [[ -d $1 ]] || die "missing directory: $1"; }

assert_json() {
  local path=$1 expression=$2
  assert_file "$path"
  python3 - "$path" "$expression" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    value = json.load(source)
helpers = {"len": len, "sum": sum, "all": all, "any": any}
if not eval(sys.argv[2], {"__builtins__": {}, **helpers}, {"value": value}):
    raise SystemExit(f"JSON assertion failed: {sys.argv[2]}")
PY
}

json_value() {
  local path=$1 expression=$2
  python3 - "$path" "$expression" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    value = json.load(source)
print(eval(sys.argv[2], {"__builtins__": {}}, {"value": value}))
PY
}
