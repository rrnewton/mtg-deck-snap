#!/usr/bin/env bash
#
# fetch-inputs.sh — populate examples/inputs/ from the shared Google Drive folder.
#
# The example INPUT images (~140MB of MTG card-spread photos) are not committed
# to git. This script downloads them into examples/inputs/ so a fresh checkout
# can run the batch pipeline against the same fixtures that produced the tracked
# examples/outputs/.
#
# Dependency: the `gdown` CLI (downloads public Google Drive folders, no auth).
# Resolution order: an existing `gdown` on PATH → uv (`uv tool run` / `uv run
# --with`) → a last-resort `pip install --user`. `make install-deps` provisions
# gdown via uv up front.
#
# gdown handles world-readable folders with no credentials. Files that already
# exist locally are skipped, so re-running is cheap and won't re-pull 140MB.

set -euo pipefail

# Public, world-readable Drive folder whose contents map to examples/inputs/
# (it holds secrets_of_strixhaven_2026/ and any future batch folders).
DRIVE_FOLDER_URL="https://drive.google.com/drive/folders/1goztYAgr5yGhfdIyIHS8kmpPLHOx7TNx"

# Resolve the destination relative to this script so it works from any CWD.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST_DIR="${SCRIPT_DIR}/inputs"

mkdir -p "${DEST_DIR}"

# Directory-level idempotency: gdown --folder has no reliable per-file skip, so
# avoid re-pulling ~140MB when inputs are already present. Set FORCE=1 to
# re-fetch anyway (e.g. FORCE=1 ./examples/fetch-inputs.sh).
if [[ -z "${FORCE:-}" ]] && find "${DEST_DIR}" -mindepth 1 -type f \
        \( -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.png' \
           -o -iname '*.heic' -o -iname '*.webp' \) -print -quit | grep -q .; then
    echo "examples/inputs/ already contains images — skipping download."
    echo "Set FORCE=1 to re-fetch anyway."
    exit 0
fi

# gdown --folder options:
#   --remaining-ok  : tolerate the folder holding more than gdown's ~50-file cap
#   --continue / -c : (via env) skip files already present
run_gdown() {
    # "$@" is the gdown invocation prefix (e.g. `gdown` or `uv run --with gdown gdown`)
    "$@" --folder "${DRIVE_FOLDER_URL}" -O "${DEST_DIR}" --remaining-ok
}

echo "Fetching example inputs into ${DEST_DIR} ..."

if command -v gdown >/dev/null 2>&1; then
    echo "Using gdown from PATH."
    run_gdown gdown
elif command -v uv >/dev/null 2>&1; then
    echo "gdown not on PATH; using uv."
    # Prefer an isolated tool install so future runs find gdown on PATH; fall
    # back to an ephemeral `uv run --with` if the tool install is unavailable.
    if uv tool install gdown >/dev/null 2>&1 && command -v gdown >/dev/null 2>&1; then
        run_gdown gdown
    else
        run_gdown uv run --with gdown gdown
    fi
else
    echo "Neither gdown nor uv found; falling back to pip install --user gdown." >&2
    python3 -m pip install --user gdown
    run_gdown gdown
fi

echo
echo "Done. examples/inputs/ now contains:"
ls -1 "${DEST_DIR}"
