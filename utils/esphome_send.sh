#!/usr/bin/env bash
# Send a fan command directly to the ESPHome bridge's web_server, bypassing
# Home Assistant. Counterpart to ha_send.sh, which goes through HA REST.
#
# Usage:
#   esphome_send.sh <fan> <command> [repeat]
#
# Examples:
#   esphome_send.sh parents_room light
#   esphome_send.sh living_room speed3 6
#
# Env (set ESPHOME_USER/PASS if web_server.auth is enabled in fan-remote.yaml):
#   ESPHOME_HOST   (default: fan-remote1.local)
#   ESPHOME_PORT   (default: 80)
#   ESPHOME_USER
#   ESPHOME_PASS

set -euo pipefail

ESPHOME_HOST="${ESPHOME_HOST:-fan-remote1.local}"
ESPHOME_PORT="${ESPHOME_PORT:-80}"

# 14-bit FAN_IDs. Keep in sync with config.yaml.
declare -A FANS=(
  [upstairs]='01111011000010'
  [living_room]='00011001111000'
  [parents_room]='01011111101000'
  [baby_room]='10001010010000'
)

# 7-bit command codes (same vocabulary for every fan; see protocol.txt).
declare -A CMDS=(
  [reverse]='0001000'
  [light]='0000010'
  [stop]='0000100'
  [speed1]='0010000'
  [speed2]='0010100'
  [speed3]='0100000'
  [speed4]='0110000'
  [speed5]='1000100'
  [speed6]='1000000'
  [pair]='0100100'
)

usage() {
  echo "usage: $0 <fan> <command> [repeat]"
  echo "  fan:     ${!FANS[*]}"
  echo "  command: ${!CMDS[*]}"
  echo "  repeat:  default 4"
  exit 2
}

[[ $# -ge 2 ]] || usage
fan="$1"; cmd="$2"; repeat="${3:-4}"

fan_id="${FANS[$fan]:-}"
cmdid="${CMDS[$cmd]:-}"
[[ -n "$fan_id" ]] || { echo "unknown fan: $fan" >&2; usage; }
[[ -n "$cmdid"  ]] || { echo "unknown command: $cmd" >&2; usage; }

bits="1111${fan_id}${cmdid}"
[[ ${#bits} -eq 25 ]] || { echo "internal: bits length ${#bits} != 25" >&2; exit 1; }

# Nonce defeats ESPHome's same-value dedup on the text entity. Keep
# it small so the whole `value=` query string stays comfortably short.
# $RANDOM is 0-32767, so up to 5 digits.
nonce="$RANDOM"
value="${bits}:${repeat}:${nonce}"

auth=()
[[ -n "${ESPHOME_USER:-}" ]] && auth=(-u "${ESPHOME_USER}:${ESPHOME_PASS:-}")

# ESPHome web_server reads `value` from the URL query string. Our value
# only contains digits and ':' separators, so the only char that needs
# percent-encoding is ':' → %3A.
encoded_value="${value//:/%3A}"
url="http://${ESPHOME_HOST}:${ESPHOME_PORT}/text/rf_tx/set?value=${encoded_value}"

echo "→ $fan / $cmd  value=${value}"
# ESPHome's IDF web_server needs Content-Length AND a Content-Type for
# POSTs — without the latter, query-string parameters can be ignored
# and the TextCall arrives with no value.
curl -sS --fail-with-body -X POST \
  -H 'Content-Length: 0' \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  "${auth[@]}" "$url"
echo
