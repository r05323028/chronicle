#!/usr/bin/env python3
"""Launch Chronicle with non-root real IDs and effective root for acceptance."""

import ctypes
import os
import sys

uid, gid = map(int, sys.argv[1:3])
if uid == 0 or os.geteuid() != 0:
    raise SystemExit("separated supervisor requires euid 0 and non-root target IDs")
libc = ctypes.CDLL(None, use_errno=True)
if libc.setresgid(gid, 0, 0) != 0 or libc.setresuid(uid, 0, 0) != 0:
    raise OSError(ctypes.get_errno(), "setresgid/setresuid")
os.execv(sys.argv[3], sys.argv[3:])
