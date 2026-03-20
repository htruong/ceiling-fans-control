This is a python daemon that exposes fan and light controls for the Casa Vieja fans to Home Assistant. The remote is called `Ceiling fan remote controller`, model is `TR301A`. The remote has a big SET button, but no DIP switches that can be configured for different fans.

Decoding the remote control signal and remote ids
--

I had to use a RTL-SDR dongle to find out the serial number for the remotes. Please follow [this wonderful tutorial](https://www.youtube.com/watch?v=_GCpqory3kc) to understand how to capture and decode the ceiling fan signal.

Sending the remote control signal
--

You can use a vanilla raspberry pi of any kind to transmit control signals (I used a Pi 0), using [rpitx](https://github.com/F5OEO/rpitx), at least in a hacky way, [without additional or customized hardware](https://www.youtube.com/watch?v=3lGU7PjJM7k).

Note that this only serves as a remote control, as we don't know what the fans are actually doing: we just send commands blindly and hope it works, just like the remotes.

Architecture
--

The daemon (`onlyfansd.py`) runs on a Raspberry Pi and bridges Home Assistant to the physical fans:

1. **Startup** — the daemon calls the HA REST API to initialise virtual fan and light entities for each configured room.
2. **Command reception** — it connects to HA over a persistent WebSocket and authenticates with a long-lived access token. It listens for `call_service` events targeting the fan/light entities.
3. **RF transmission** — when a command arrives, it invokes `rpitx` via `control_fan.py` to transmit the corresponding RF signal on the Pi's GPIO pin, mimicking the original remote.
4. **State updates** — after each command, it pushes the new state back to HA via the REST API so the UI reflects the change.

This approach requires no external broker — the Pi talks directly to HA over WebSocket and REST.

How to use this repository
--

Install prereqs:

```
sudo apt install python3-yaml
pip3 install requests websocket-client
```

Copy `config_sample.yaml` to `config.yaml` and fill in:
- Your HA URL and a long-lived access token (Profile → Long-Lived Access Tokens in HA)
- The RF remote ID for each fan (see the decoding section above)

Then run the daemon:

```
python3 onlyfansd.py
```

Adding fans to Home Assistant
--

The daemon creates entities via the HA REST API (`/api/states/`), which means they appear in **Developer Tools → States** but are not backed by an integration — HA will not auto-add them to your dashboards.

To make them controllable from the UI, add them manually to a Lovelace dashboard:

1. Edit a dashboard → Add Card → Entities
2. Add the fan entities: `fan.{room}_fan`
3. Add the light entities: `light.{room}_fan_light`

The states reported are best-effort (the daemon tracks what it last commanded, not what the fan is actually doing), so treat them as approximate.

If you can't control the fans, debug by comparing the signal that this daemon sends out against the actual signal the remote sends out to understand what went wrong. Or you could spend some more minutes to figure out how the SET button works, and then add a program button to the integration. I can't be bothered to figure out how the SET button works, to send arbitrary IDs we choose to the fans, but [I assume that's possible](https://www.amazon.com/review/R2VWOTH0LUT4XJ/).
