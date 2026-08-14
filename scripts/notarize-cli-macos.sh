#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <submission.zip> <signed-binary>" >&2
  exit 2
fi

submission_archive=$1
signed_binary=$2

for required_name in APPLE_ID APPLE_APP_SPECIFIC_PASSWORD APPLE_TEAM_ID; do
  if [[ -z "${!required_name:-}" ]]; then
    echo "Missing required environment variable: $required_name" >&2
    exit 2
  fi
done

if [[ ! -f "$submission_archive" || ! -f "$signed_binary" ]]; then
  echo "The notarization archive and signed binary must both exist." >&2
  exit 2
fi

timeout_minutes=${NOTARIZATION_TIMEOUT_MINUTES:-90}
if [[ ! "$timeout_minutes" =~ ^[1-9][0-9]*$ ]]; then
  echo "NOTARIZATION_TIMEOUT_MINUTES must be a positive integer." >&2
  exit 2
fi

plutil_bin=${PLUTIL_BIN:-/usr/bin/plutil}

poll_seconds=30
heartbeat_seconds=300
timeout_seconds=$((timeout_minutes * 60))
started_at=0
next_heartbeat_at=0
previous_status=
consecutive_failures=0
submission_id=
accepted=false

submission_plist=$(mktemp)
info_plist=$(mktemp)
trap 'rm -f "$submission_plist" "$info_plist"' EXIT

authorization=(
  --apple-id "$APPLE_ID"
  --password "$APPLE_APP_SPECIFIC_PASSWORD"
  --team-id "$APPLE_TEAM_ID"
)

echo "Uploading signed CLI to Apple's notarization service..."
xcrun notarytool submit \
  "$submission_archive" \
  "${authorization[@]}" \
  --no-wait \
  --output-format plist >"$submission_plist"

submission_id=$("$plutil_bin" -extract id raw -o - "$submission_plist")
if [[ -z "$submission_id" ]]; then
  echo "Apple accepted the upload but returned no submission ID." >&2
  exit 1
fi
echo "Apple notarization submitted: $submission_id"
started_at=$SECONDS

while ((SECONDS - started_at < timeout_seconds)); do
  if xcrun notarytool info \
    "$submission_id" \
    "${authorization[@]}" \
    --output-format plist >"$info_plist"; then
    consecutive_failures=0
  else
    consecutive_failures=$((consecutive_failures + 1))
    if ((consecutive_failures >= 3)); then
      echo "Unable to read Apple notarization $submission_id after 3 attempts." >&2
      exit 1
    fi
    echo "Could not read Apple notarization $submission_id; retrying ($consecutive_failures/3)."
    sleep "$poll_seconds"
    continue
  fi

  status=$("$plutil_bin" -extract status raw -o - "$info_plist")
  elapsed_seconds=$((SECONDS - started_at))
  if [[ "$status" != "$previous_status" ]] || ((elapsed_seconds >= next_heartbeat_at)); then
    echo "Apple notarization $submission_id: $status ($((elapsed_seconds / 60))m elapsed)"
    previous_status=$status
    next_heartbeat_at=$((elapsed_seconds + heartbeat_seconds))
  fi

  case "$status" in
    Accepted)
      accepted=true
      break
      ;;
    "In Progress")
      sleep "$poll_seconds"
      ;;
    *)
      echo "Apple notarization $submission_id finished with status: $status" >&2
      xcrun notarytool log "$submission_id" "${authorization[@]}" || true
      exit 1
      ;;
  esac
done

if [[ "$accepted" != true ]]; then
  echo "Apple notarization $submission_id timed out after $timeout_minutes minutes." >&2
  echo "Apple may continue processing it; inspect the submission with notarytool info." >&2
  exit 1
fi

# A bare Mach-O executable has no container to staple and is not an app bundle
# that spctl can assess. codesign's online check verifies the notarization ticket
# for the executable itself; retry briefly while Apple's result propagates.
verification=
for attempt in 1 2 3 4 5 6; do
  if verification=$(
    codesign --verify --strict --verbose=4 --check-notarization "$signed_binary" 2>&1
  ); then
    printf '%s\n' "$verification"
    echo "Apple notarization accepted and ticket verified: $submission_id"
    exit 0
  fi
  if ((attempt < 6)); then
    echo "codesign has not observed ticket $submission_id yet; retrying ($attempt/6)."
    sleep 10
  fi
done

printf '%s\n' "$verification" >&2
echo "codesign could not verify notarization ticket $submission_id." >&2
exit 1
