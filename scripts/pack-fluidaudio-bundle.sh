#!/usr/bin/env bash
# Build a Blue Ear fluidaudio-v1 model bundle from locally cloned Hugging Face
# repos. Blue Ear never downloads these itself -- you fetch them in a browser
# (or with git lfs), then run this script, then Import the output folder.
#
# Usage:
#   scripts/pack-fluidaudio-bundle.sh \
#     /path/to/parakeet-tdt-0.6b-v3-coreml \
#     /path/to/speaker-diarization-coreml \
#     [/path/to/output-parent]
#
# Writes <output-parent>/fluidaudio-v1/ with the two model directories and a
# manifest.json Blue Ear will validate on import. Defaults output-parent to
# the current working directory.
#
# The ASR directory is renamed from the Hugging Face slug
# (`parakeet-tdt-0.6b-v3-coreml`) to FluidAudio's local cache name
# (`parakeet-tdt-0.6b-v3`, `-coreml` stripped). AsrModels.load resolves models
# under Repo.folderName, which uses that stripped name.

set -euo pipefail

ASR_HF_NAME="parakeet-tdt-0.6b-v3-coreml"
ASR_CACHE_NAME="parakeet-tdt-0.6b-v3"
DIAR_NAME="speaker-diarization-coreml"
BUNDLE_ID="fluidaudio-v1"
SDK_VERSION="0.15.5"
DISPLAY_NAME="Parakeet v3 and diarizer"

usage() {
  echo "usage: $0 <asr-repo-dir> <diarization-repo-dir> [output-parent]" >&2
  echo "  asr-repo-dir must be (or contain) ${ASR_HF_NAME} or ${ASR_CACHE_NAME}" >&2
  echo "  diarization-repo-dir must be (or contain) ${DIAR_NAME}" >&2
  exit 1
}

[[ $# -ge 2 && $# -le 3 ]] || usage

resolve_repo() {
  local input="$1"
  shift
  local expected
  for expected in "$@"; do
    if [[ -d "${input}/${expected}" ]]; then
      echo "${input}/${expected}"
      return 0
    elif [[ -d "${input}" && "$(basename "${input}")" == "${expected}" ]]; then
      echo "${input}"
      return 0
    fi
  done
  echo "error: expected a directory named one of [$*] (or a parent containing it): ${input}" >&2
  exit 1
}

ASR_SRC="$(resolve_repo "$1" "${ASR_HF_NAME}" "${ASR_CACHE_NAME}")"
DIAR_SRC="$(resolve_repo "$2" "${DIAR_NAME}")"
OUT_PARENT="${3:-.}"
OUT_DIR="${OUT_PARENT%/}/${BUNDLE_ID}"

if [[ -e "${OUT_DIR}" ]]; then
  echo "error: output already exists: ${OUT_DIR}" >&2
  exit 1
fi

# Reject symlinks in the source trees: Blue Ear's importer does the same, and
# packing them would just produce a bundle that fails on import.
reject_symlinks() {
  local root="$1"
  local found
  found="$(find "${root}" -type l -print -quit)"
  if [[ -n "${found}" ]]; then
    echo "error: refusing to pack a tree that contains symlinks (found ${found})" >&2
    exit 1
  fi
}

reject_symlinks "${ASR_SRC}"
reject_symlinks "${DIAR_SRC}"

mkdir -p "${OUT_DIR}"
# Copy without following links; -R on macOS does not dereference by default,
# and we already rejected symlinks above. Always stage ASR under FluidAudio's
# cache folder name so ModelHub finds it after AsrModels.load strips -coreml.
cp -R "${ASR_SRC}" "${OUT_DIR}/${ASR_CACHE_NAME}"
cp -R "${DIAR_SRC}" "${OUT_DIR}/${DIAR_NAME}"

# Strip VCS metadata and Hugging Face cache noise from the staged copy so the
# manifest inventory matches what FluidAudio actually loads.
find "${OUT_DIR}" \( \
  -name '.git' -o -name '.gitattributes' -o -name '.gitignore' -o \
  -name '.cache' -o -name '.DS_Store' -o -name '*.md' \
\) -prune -exec rm -rf {} + 2>/dev/null || true

# Walk every regular file, emit "sha256 size relative-path" lines. Paths are
# relative to OUT_DIR and use forward slashes.
INVENTORY="$(mktemp)"
trap 'rm -f "${INVENTORY}"' EXIT

(
  cd "${OUT_DIR}"
  # LC_ALL=C keeps sort order stable across locales.
  find . -type f -print0 | LC_ALL=C sort -z | while IFS= read -r -d '' rel; do
    rel="${rel#./}"
    # Skip nothing else: every remaining file must be declared.
    size="$(stat -f%z "${rel}")"
    digest="$(shasum -a 256 "${rel}" | awk '{print $1}')"
    printf '%s %s %s\n' "${digest}" "${size}" "${rel}"
  done
) > "${INVENTORY}"

if [[ ! -s "${INVENTORY}" ]]; then
  echo "error: no files found to pack" >&2
  rm -rf "${OUT_DIR}"
  exit 1
fi

python3 - "${INVENTORY}" "${OUT_DIR}/manifest.json" \
  "${ASR_CACHE_NAME}" "${DIAR_NAME}" "${SDK_VERSION}" "${DISPLAY_NAME}" <<'PY'
import json
import sys
from pathlib import Path

inventory_path = Path(sys.argv[1])
manifest_path = Path(sys.argv[2])
asr_id = sys.argv[3]
diar_id = sys.argv[4]
sdk_version = sys.argv[5]
display_name = sys.argv[6]

files = []
for line in inventory_path.read_text().splitlines():
    digest, size, path = line.split(" ", 2)
    files.append({
        "path": path,
        "sizeBytes": int(size),
        "sha256": digest,
    })

manifest = {
    "schemaVersion": 1,
    "bundleId": "fluidaudio-v1",
    "displayName": display_name,
    "provider": "fluidaudio",
    "sdkVersion": sdk_version,
    "models": [
        {
            "id": asr_id,
            "role": "asr",
            "license": "CC-BY-4.0",
        },
        {
            "id": diar_id,
            "role": "diarization",
            "license": "see model card",
        },
    ],
    "files": files,
}

manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
print(f"wrote {manifest_path} ({len(files)} files)")
PY

echo "Blue Ear bundle ready: ${OUT_DIR}"
echo "Import this folder in Blue Ear → Transcription → Import model bundle."
echo "Selecting FluidAudio as the provider is a separate step after import."
