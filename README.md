A bridge that exposes Casa Vieja ceiling fans (remote model `TR301A`) to
Home Assistant via an ESP32 + CC1101 RF radio. The remote has a big SET
button but no DIP switches, so we pair by sniffing the remote's unique
14-bit `FAN_ID` off the air and replaying it.

Two pieces, use either or both:

1. **ESPHome + CC1101 + HA** (the RF bridge — required). One
   ESP32-based RF node; all per-fan logic lives in Home Assistant;
   no firmware reflash to experiment. See below.
2. **`onlyfansd`** (Rust, optional). A small daemon that adds a native
   HomeKit bridge and HA entity init on top of #1. It sends RF through
   the same ESPHome path via HA REST. See [Optional: onlyfansd](#optional-onlyfansd-homekit-bridge-daemon)
   at the bottom.

Architecture (ESPHome path)
--

```
                            304.30 MHz OOK
  ┌───────────┐ ────────────────────────────────►  Ceiling fans
  │  ESP32 +  │                                   (no ACK, dumb TX)
  │  CC1101   │ ◄───── WiFi ─────────────────►  Home Assistant / curl
  └───────────┘                                  / Rust daemon / …
  fan-remote.yaml
  (deployed once,
   no per-fan state)
```

The ESP32 firmware (`fan-remote.yaml`) is a **dumb RF transmitter** with
no per-fan knowledge: it accepts an arbitrary `'0'/'1'` bit string over
two transports (HA-native action, plain HTTP via `web_server`) and OOK-
encodes it on the fly. Both transports share one `script: do_transmit`
that runs the encoder.

Adding a new fan or changing a command never requires a reflash:

| Change                | Touch                          | Reflash? |
| --------------------- | ------------------------------ | -------- |
| Add a new TX command  | HA script / `ha_send.sh` / `esphome_send.sh` | No |
| Add a fan to control  | HA script / `ha_send.sh` / `esphome_send.sh` | No |

Hardware
--

ESP32 (esp32dev / DevKit-C) + CC1101 sub-GHz module. TX-only — GDO2 is
not connected.

| ESP32 GPIO | CC1101 pin | Note                              |
| ---------- | ---------- | --------------------------------- |
| GPIO18     | SCK        |                                   |
| GPIO23     | MOSI       |                                   |
| GPIO19     | MISO       |                                   |
| GPIO5      | CSN        | strapping pin — harmless warning  |
| GPIO22     | GDO0       | TX data into CC1101               |
| 3V3, GND   | VCC, GND   |                                   |

Antenna: a 24.6 cm wire on the ANT pad works fine at 304.30 MHz; a tuned
helical or whip is better if range is tight.

Flash the firmware
--

1. Provide a `secrets.yaml` next to `fan-remote.yaml` with `wifi_ssid`,
   `wifi_password`, `api_encryption_key`, `ota_password`, `ap_password`,
   `web_username`, `web_password`.
2. `esphome run fan-remote.yaml`. ESPHome's built-in CC1101 component
   (since [esphome/esphome#11849](https://github.com/esphome/esphome/pull/11849))
   is used — no `external_components` needed.
3. Verify in Home Assistant → Settings → Devices → ESPHome that
   `fan_remote1` is online.

To learn a new fan's 14-bit FAN_ID, either sniff one of its remotes
with an RTL-SDR + `utils/decode_fan_remote.py`, or pair an arbitrary ID
via the SET-button procedure below.

Sending a fan command
--

`fan-remote.yaml` accepts the same RF payload over two transports
simultaneously — they both run through one shared encoder script, so
behaviour is identical, you just pick the wire format that suits the
caller.

**Transport A: HA-native action** (HA must be in the loop)

```yaml
# HA automation / Developer Tools → Actions
service: esphome.fan_remote1_transmit_fan_bits
data:
  bits: "1111010111111010000000010"   # 4 preamble + 14 fan_id + 7 cmdid
  repeat: 4
```

Or from a shell via HA REST:

```
utils/ha_send.sh parents_room light
utils/ha_send.sh living_room speed3 6       # repeat 6 times
HA_URL=http://10.0.0.5:8123 utils/ha_send.sh upstairs stop
```

**Transport B: plain HTTP** (bypasses HA — works even if HA is down)

The ESPHome `web_server` exposes a `text` entity named `rf_tx`.
POSTing to `/text/rf_tx/set` with `value=<bits>[:<repeat>[:<nonce>]]`
fires the same encoder. `web_server.auth` is on; supply HTTP Basic
credentials from `secrets.yaml`.

From a shell, the analogue to `ha_send.sh` (already auto-appends a
nanosecond nonce so back-to-back identical commands don't get
deduped):

```
ESPHOME_USER=admin ESPHOME_PASS=secret \
  utils/esphome_send.sh parents_room light
ESPHOME_HOST=192.168.2.217 utils/esphome_send.sh living_room speed3 6
```

Or hand-rolled:

```
curl -u "$WEB_USER:$WEB_PASS" -X POST \
  --data-urlencode "value=1111010111111010000000010:4:$(date +%s%N)" \
  http://fan-remote1.local/text/rf_tx/set
```

The encoder is bit-length agnostic, so other fans / OOK protocols can
ride the same firmware: pass any `'0'/'1'` string.

Pairing a fan with an arbitrary ID
--

Per the remote's manual, the fan learns whichever 14-bit `FAN_ID` the
remote transmits during a pairing window:

1. Remove the battery cover on the remote — there's a "Learn Switch"
   underneath.
2. Cut power to the ceiling fan at the breaker (or wall switch) for
   one minute.
3. Restore power. You have 60 seconds to pair.
4. Press and hold the Learn Switch with a pen or paperclip until the
   light on the fan blinks twice, then release.

You can also send the `pair` command (`0100100`) from this bridge
with your chosen 14-bit FAN_ID during the pairing window — the fan
should latch onto it. I've never actually tried this; I sniffed my
existing remotes instead. If you try it, please open an issue.

Decoding an existing remote's ID
--

If you'd rather mirror an existing remote than assign a new ID, either:

- Capture with an RTL-SDR dongle (follow
  [this tutorial](https://www.youtube.com/watch?v=_GCpqory3kc)) and
  decode with `utils/decode_fan_remote.py`, or
- Temporarily add a `remote_receiver:` + `binary_sensor:` block to
  `fan-remote.yaml` (wire GDO2 → an ESP GPIO), set `dump: raw`, press
  the unknown remote, and read the 25-bit decoded message from the
  ESPHome `Received Raw:` log lines. Then strip the RX block back out.

Protocol
--

304.30 MHz OOK, 333 µs per chip, 25-bit messages = preamble(4) +
FAN_ID(14) + cmdid(7). Each logical bit `b` encodes as 3 chips: `'1',
'0', b`. Full details in [protocol.txt](protocol.txt).

Repository layout
--

```
fan-remote.yaml                 ESPHome firmware for the ESP32+CC1101 bridge.
                                Deploy once. Exposes two transports for
                                the `do_transmit` script.
utils/
  ha_send.sh                    Shell wrapper that goes via HA REST →
                                esphome.fan_remote1_transmit_fan_bits.
  esphome_send.sh               Shell wrapper that goes directly to the
                                ESPHome web_server at /text/rf_tx/set,
                                bypassing HA.
  config.yaml                   14-bit FAN_IDs per room. Used by the
                                onlyfansd daemon and the Python ref impl.
  decode_fan_remote.py          Offline decoder for "Received Raw:" log
                                lines. Useful when sniffing a new remote.
  control_fan.py                Original Python reference TX implementation
                                (uses sendook on a Pi — pre-ESPHome).
  homekit_bridge.py             Original Python HomeKit bridge (pre-onlyfansd).
protocol.txt                    RF protocol reference (the meat).
ceiling_fans.yaml               HA `template:` include for fan + light
                                entities (used by the onlyfansd daemon).
src/                            onlyfansd (Rust daemon). See section below.
```

TX timing notes (lessons learned)
--

The transmitter is configured `non_blocking: true` and the encoder
appends ~30 chips of silence (~10 ms) to the end of each frame. That
means each `transmit_raw` call hands the RMT peripheral one
self-contained pulse vector (data + trailing pause), and the `repeat:`
in `do_transmit` queues N of them back-to-back.

This matters: an earlier version used `non_blocking: false` with a
separate `- delay: 10ms` action between repeats. That worked when the
ESP was idle but the inter-burst gap was at the mercy of the action-
chain scheduler — under WiFi / API / web_server load, the gap would
jitter from 8 ms to 15+ ms and the fans (whose decoders are picky
about timing) would silently ignore most commands. Moving the silence
*into the pulse list* puts the timing entirely on the RMT hardware
clock; jitter goes to zero and reliability goes from "one in twenty"
back to 100%.

Other knobs that earned their keep:
- `cc1101.set_idle` before `cc1101.begin_tx` (in `on_transmit`) forces
  a fresh PLL calibration per frame via the idle→tx transition.
- `symbol_rate: 3000` matches our actual 333 µs chip duration. The
  CC1101's async-serial OOK mode uses symbol_rate for the modulator's
  internal oversampling; a 60 % mismatch (we had 4800) was probably
  smearing edges.
- `bluetooth_proxy:` / `esp32_ble_tracker:` are intentionally absent.
  Even with `active: false` they keep the BLE stack live and we saw
  reliability and resource issues.

Debugging
--

The fan didn't respond:

- Check ESPHome logs at INFO level for `[fan-remote]: TX bits=...` —
  that's emitted from inside the encoder lambda, so seeing it confirms
  the script ran. If you don't see it, the call never reached the
  device (check HA → ESPHome / web_server auth / network).
- If you see `TX bits=...` but no fan response: try a closer range to
  rule out RF link budget; bump `output_power` (max 11 dBm); check
  antenna seating; verify the 3.3 V rail is steady under load.
- If you suspect the fan lost its pairing (e.g. after a long power
  outage), re-pair it with the SET-button procedure above.

Optional: onlyfansd (HomeKit bridge daemon)
--

`onlyfansd` (Rust, in `src/`) is an optional layer on top of the
ESPHome+HA setup. It adds:

- A native **HomeKit** bridge (vendored `hap-rs`) so iOS Home can
  control the fans directly via HAP, without going through HA.
- HA entity initialisation (creates `fan.<room>_fan` and
  `light.<room>_fan_light` states via REST so HA dashboards see them
  without needing the `ceiling_fans.yaml` template include).
- Speed snapping (HA's 0–100 % → 6 discrete speeds) and a 1.5 s
  debounce on speed changes so sliding the HA slider doesn't fire
  six RF transmissions.
- Persisted state in `fan_state.json` across restarts.

It no longer transmits RF directly — all sends ride the ESPHome bridge.
The daemon picks one of two transports via `esphome.transport`:

| transport | path                                                | when to use                                            |
| --------- | --------------------------------------------------- | ------------------------------------------------------ |
| `ha_rest` | daemon → HA REST → ESPHome native API               | default; HA logs every TX in the event bus             |
| `http`    | daemon → ESPHome web_server `/text/rf_tx/set`       | bypasses HA — HomeKit keeps working if HA is down      |

```
                                ha_rest:
                              ┌─────────────────► Home Assistant ─────┐
HA WebSocket  ─┐              │                                       ▼
               ├─►  onlyfansd ┤                                   fan-remote1
HomeKit (HAP) ─┘              │   http (direct, with HA bypassed):    ▲
                              └───────────────────────────────────────┘
```

Either transport reaches the same `do_transmit` script on the device.
For `http`, the daemon appends a monotonic nonce to defeat any
same-value dedup on the ESPHome `text` entity. Config:

```yaml
esphome:
  device: fan_remote1
  repeat: 4
  transport: http        # or ha_rest (default)
  http:                  # required when transport: http
    host: fan-remote1.local
    port: 80
    username: !secret web_user
    password: !secret web_pass
```

Build & deploy (pure Rust, no native deps):

```
cargo build --release
# or for a Pi: cargo zigbuild --release --target arm-unknown-linux-gnueabihf
# copy target/.../onlyfansd to /usr/local/bin/onlyfansd
# copy config to /etc/onlyfansd/config.yaml (template: config_sample.yaml)
# run under systemd.
```

Set `homeassistant.listen: false` in the config for a HomeKit-only
deployment (no WS subscription, no entity-state mirroring to HA).
Pair this with `esphome.transport: http` for the most HA-independent
setup the daemon supports.

See `src/main.rs`, `config_sample.yaml`, and `ceiling_fans.yaml`.
