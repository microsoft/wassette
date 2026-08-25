#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

"""Line-oriented stdio tap.

Copies stdin to stdout unchanged while appending a timestamped copy of every
line to the file named by argv[1]. Timestamps are epoch seconds with
microsecond resolution so the two directions of a JSON-RPC conversation can be
merged and ordered exactly, which is what lets a run say whether the client's
tools/list came *after* the server's notifications/tools/list_changed.
"""

import sys
import time


def main() -> int:
    path = sys.argv[1]
    with open(path, "a", buffering=1, errors="replace") as log:
        for line in sys.stdin:
            log.write(
                f"{time.time():.6f}\t"
                f"{line if line.endswith(chr(10)) else line + chr(10)}"
            )
            sys.stdout.write(line)
            sys.stdout.flush()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (BrokenPipeError, KeyboardInterrupt):
        sys.exit(0)
