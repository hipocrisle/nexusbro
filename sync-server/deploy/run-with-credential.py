#!/usr/bin/env python3
"""Read a systemd encrypted credential without emitting it, then exec the server."""
import base64
import os
from pathlib import Path
import sys

credential_dir = os.environ.get("CREDENTIALS_DIRECTORY")
if not credential_dir:
    sys.exit("Systemd credential directory is required")
try:
    secret = (Path(credential_dir) / "totp-key").read_text().strip()
    if len(base64.b64decode(secret, validate=True)) != 32:
        raise ValueError("length")
except (OSError, ValueError):
    sys.exit("A valid independent TOTP deployment key is required")
os.environ["NEXUSBRO_SYNC_TOTP_KEY"] = secret
os.execv("/opt/nexusbro-sync/nexusbro-sync-server", ["nexusbro-sync-server"])
