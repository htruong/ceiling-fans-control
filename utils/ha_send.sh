#!/usr/bin/env bash
# Send a fan command via Home Assistant's REST API, which proxies it to
# the ESPHome fan-remote1 node via the `transmit_fan_bits` action.
#
# Usage:
#   ha_send.sh <fan> <command> [repeat]
#
# Examples:
#   ha_send.sh parents_room light
#   ha_send.sh living_room speed3 6
#
# Env (HA_TOKEN required, others optional):
#   HA_TOKEN    Long-lived HA access token. Required.
#               Create one under HA → Your Profile → Security → Long-lived
#               access tokens. Stash it in your shell profile or in
#               ~/.config/onlyfansd/env and `source` it.
#   HA_URL      (default: http://homeassistant:8123)
#   HA_DEVICE   (default: fan_remote1 — the underscore form of the esphome name)

set -euo pipefail

HA_URL="${HA_URL:-http://homeassistant:8123}"
HA_DEVICE="${HA_DEVICE:-fan_remote1}"

if [[ -z "${HA_TOKEN:-}" ]]; then
  echo "error: HA_TOKEN env var must be set (long-lived HA access token)" >&2
  exit 2
fi

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

echo "→ $fan / $cmd  bits=$bits repeat=$repeat"
curl -sS --fail-with-body -X POST \
  -H "Authorization: Bearer $HA_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"bits\":\"$bits\",\"repeat\":$repeat}" \
  "$HA_URL/api/services/esphome/${HA_DEVICE}_transmit_fan_bits"
echo
