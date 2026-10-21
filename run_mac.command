#!/bin/bash
# Duet launcher for macOS. Run it with:   bash ~/Downloads/duet/run_mac.command
# First run sets up a private Python environment (~1 minute), later runs start right away.
set -e
cd "$(dirname "$0")"

if ! command -v python3 >/dev/null 2>&1; then
    echo "Python isn't installed. Get it from https://www.python.org/downloads/macos/ and run this again."
    exit 1
fi

if [ ! -x .venv/bin/python ]; then
    echo "First run: setting things up..."
    python3 -m venv .venv
    .venv/bin/pip install --quiet --upgrade pip
    .venv/bin/pip install --quiet -r requirements.txt
fi

exec .venv/bin/python duet.py "$@"
