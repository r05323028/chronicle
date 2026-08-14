#!/usr/bin/env python3
"""Write the OpenCode Go custom provider into ~/.pi/agent/models.json.

Environment-driven, so the workflow owns the values and this script never
hardcodes them:
  OPENCODE_GO_BASE_URL  -> provider baseUrl (non-secret workflow env)
  RELEASE_NOTES_MODEL   -> provider model id (non-secret workflow env)

The apiKey is written as the literal env-var-name reference "$OPENCODE_GO_API_KEY";
pi resolves it from the process environment at request time. The secret value
itself is never interpolated into this file. Fail closed on missing env vars.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path


def main() -> int:
    base_url = os.environ.get("OPENCODE_GO_BASE_URL", "").strip()
    model = os.environ.get("RELEASE_NOTES_MODEL", "").strip()
    if not base_url:
        print("OPENCODE_GO_BASE_URL must be set", file=sys.stderr)
        return 1
    if not model:
        print("RELEASE_NOTES_MODEL must be set", file=sys.stderr)
        return 1

    config = {
        "providers": {
            "opencode-go": {
                "baseUrl": base_url,
                "api": "openai-completions",
                "apiKey": "$OPENCODE_GO_API_KEY",
                "authHeader": True,
                "models": [{"id": model}],
            }
        }
    }

    out = Path.home() / ".pi" / "agent" / "models.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8") as fh:
        json.dump(config, fh, indent=2)
        fh.write("\n")
    print(f"wrote {out} (provider opencode-go, model {model})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
