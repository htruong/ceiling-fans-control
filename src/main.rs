mod homekit;

#[cfg(target_os = "linux")]
mod cc1101;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Config {
    homeassistant: HaConfig,
    rf: RfConfig,
    // Required when rf.transport ∈ {ha_rest, http}.
    #[serde(default)]
    esphome: Option<EspHomeConfig>,
    // Required when rf.transport = spi.
    #[serde(default)]
    cc1101: Option<Cc1101Config>,
    fans: HashMap<String, String>, // room → 14-bit fan ID string
    #[serde(default)]
    homekit: Option<HomeKitConfig>,
}

#[derive(Deserialize)]
struct HaConfig {
    url: String,
    token: String,
    // Whether to subscribe to HA's WebSocket for `call_service` events and
    // mirror entity state back to HA via REST. The outbound RF path
    // (HA → ESPHome) always uses the REST API regardless of this setting.
    #[serde(default = "default_true")]
    listen: bool,
}

#[derive(Deserialize)]
struct RfConfig {
    // How the daemon emits the 25-bit OOK frame:
    //   ha_rest (default) — POST to HA's REST API, which proxies the
    //     `transmit_fan_bits` action over HA's native ESPHome link.
    //     Requires the `esphome:` block.
    //   http              — POST directly to the ESPHome device's
    //     web_server REST endpoint. Bypasses HA. Requires the
    //     `esphome:` block (with an `esphome.http:` sub-block).
    //   spi               — drive a locally-attached CC1101 over SPI
    //     (Pi-native). No ESPHome involved. Requires the `cc1101:`
    //     block and a Linux build.
    #[serde(default = "default_transport")]
    transport: Transport,
    // Retransmissions per command. The physical remote sends 3-5;
    // 4 is a fine default.
    #[serde(default = "default_repeat")]
    repeat: u32,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Transport {
    HaRest,
    Http,
    Spi,
}

#[derive(Deserialize)]
struct EspHomeConfig {
    // ESPHome device name (underscored form). Used as the HA service
    // suffix on the ha_rest transport.
    device: String,
    // Required for transport: http; ignored for transport: ha_rest.
    #[serde(default)]
    http: Option<EspHomeHttpConfig>,
}

#[derive(Deserialize)]
struct EspHomeHttpConfig {
    // Hostname or IP of the ESPHome device. e.g. "fan-remote1.local"
    // or "192.168.2.217".
    host: String,
    #[serde(default = "default_http_port")]
    port: u16,
    // HTTP Basic auth for /text/rf_tx/set — match web_server.auth in
    // fan-remote.yaml. Omit both if you left web_server.auth unset.
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Cc1101Config {
    // SPI character device. On a Raspberry Pi with the default overlay,
    // SPI0 with CE0 selected is /dev/spidev0.0.
    #[serde(default = "default_spi_device")]
    device: String,
    // Carrier frequency in Hz. Default targets Casa Vieja TR301A
    // (304.30 MHz). The driver computes the FREQ register from this.
    #[serde(default = "default_frequency_hz")]
    frequency_hz: u32,
    // CC1101 PATABLE[1] byte — the "carrier on" PA setting in OOK mode.
    // 0xC0 is the canonical "max power, 315 MHz band" entry (~+11 dBm).
    // Lower values trade range for current draw and spectral cleanliness.
    #[serde(default = "default_power")]
    power: u8,
}

fn default_true() -> bool { true }
fn default_repeat() -> u32 { 4 }
fn default_transport() -> Transport { Transport::HaRest }
fn default_http_port() -> u16 { 80 }
fn default_spi_device() -> String { "/dev/spidev0.0".into() }
fn default_frequency_hz() -> u32 { 304_300_000 }
fn default_power() -> u8 { 0xC0 }

#[derive(Deserialize)]
struct HomeKitConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_hk_port")]
    port: u16,
    #[serde(default = "default_hk_persist")]
    persist_dir: String,
    #[serde(default = "default_hk_pin")]
    pin: String,  // 8 digits, e.g. "03141592"
    #[serde(default)]
    name: Option<String>,
    // Address the HAP TCP listener binds to. Default 0.0.0.0 listens on
    // every interface, which is what most embedded HAP bridges do — pin
    // it to a specific NIC IP only if you've got a multi-homed host and
    // need to constrain which network can see the accessory. Defaulting
    // to 0.0.0.0 also avoids the boot race where hap-rs's
    // `redetermine_local_ip()` runs before DHCP completes, finds only
    // loopback, and persists `"host":"127.0.0.1"` into the config —
    // after which the HAP listener is unreachable from the LAN and
    // every iOS pairing attempt times out.
    #[serde(default = "default_hk_bind")]
    bind: String,
}

fn default_hk_port() -> u16 { 51826 }
fn default_hk_persist() -> String { "homekit".into() }
fn default_hk_pin() -> String { "03141592".into() }
fn default_hk_bind() -> String { "0.0.0.0".into() }

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "onlyfansd".into())
}

fn parse_pin(s: &str) -> [u8; 8] {
    let digits: Vec<u8> = s.chars().filter_map(|c| c.to_digit(10).map(|d| d as u8)).collect();
    let mut out = [0u8; 8];
    for (i, d) in digits.iter().take(8).enumerate() { out[i] = *d; }
    out
}

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
struct FanState {
    state: String,
    percentage: u32,
    #[serde(default = "default_direction")]
    direction: String,
}

fn default_direction() -> String {
    "forward".to_string()
}

impl Default for FanState {
    fn default() -> Self {
        Self { state: "OFF".into(), percentage: 0, direction: "forward".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct LightState {
    state: String,
}

impl Default for LightState {
    fn default() -> Self {
        Self { state: "OFF".into() }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedState {
    fans: HashMap<String, FanState>,
    lights: HashMap<String, LightState>,
}

struct SharedState {
    fan_states: HashMap<String, FanState>,
    light_states: HashMap<String, LightState>,
    pending_timers: HashMap<String, tokio::task::AbortHandle>,
}

impl SharedState {
    fn new() -> Self {
        Self { fan_states: HashMap::new(), light_states: HashMap::new(), pending_timers: HashMap::new() }
    }
}

// ── RF Protocol ───────────────────────────────────────────────────────────────

// 7-bit cmdid vocabulary — same for every fan. See protocol.txt.
// The physical-layer encoding (chips, pulse timing, 304.30 MHz OOK) lives
// in the ESPHome firmware; the daemon only assembles the 25-bit message.
const CMDS: &[(&str, &str)] = &[
    ("reverse", "0001000"),
    ("light",   "0000010"),
    ("stop",    "0000100"),
    ("speed1",  "0010000"),
    ("speed2",  "0010100"),
    ("speed3",  "0100000"),
    ("speed4",  "0110000"),
    ("speed5",  "1000100"),
    ("speed6",  "1000000"),
    ("pair",    "0100100"),
];

// ── Persistence ───────────────────────────────────────────────────────────────

const STATE_FILE: &str = "fan_state.json";

fn save_state(s: &SharedState) {
    let p = PersistedState { fans: s.fan_states.clone(), lights: s.light_states.clone() };
    if let Ok(json) = serde_json::to_string_pretty(&p) {
        if let Err(e) = std::fs::write(STATE_FILE, json) {
            error!("save state: {}", e);
        }
    }
}

fn load_state() -> PersistedState {
    match std::fs::read_to_string(STATE_FILE) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => PersistedState::default(),
        Err(e) => { error!("load state: {}", e); PersistedState::default() }
    }
}

// ── Home Assistant REST ───────────────────────────────────────────────────────

async fn ha_set_state(client: &Client, url: &str, token: &str, entity_id: &str, state: &str, attrs: Value) {
    let endpoint = format!("{}/api/states/{}", url, entity_id);
    match client.post(&endpoint)
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "state": state, "attributes": attrs }))
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => error!("HA {} → {}", entity_id, r.status()),
        Err(e) => error!("HA {} failed: {}", entity_id, e),
    }
}

fn title_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Daemon ────────────────────────────────────────────────────────────────────

// Pi-native CC1101 handle. Wrapped in a std Mutex since the underlying
// spidev calls are blocking and we serialise transmits via spawn_blocking.
// Aliased so the Daemon struct compiles on non-Linux (where the handle is
// always None and the SPI code path is cfg'd out).
#[cfg(target_os = "linux")]
type Cc1101Handle = Arc<std::sync::Mutex<cc1101::Cc1101>>;
#[cfg(not(target_os = "linux"))]
type Cc1101Handle = ();

struct Daemon {
    config: Config,
    client: Client,
    state: Mutex<SharedState>,
    homekit: Option<Arc<homekit::HomeKit>>,
    // Local CC1101 radio, present only when rf.transport = spi (and only
    // ever Some on Linux builds).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    radio: Option<Cc1101Handle>,
    // Monotonic counter appended to HTTP-transport payloads so two
    // consecutive identical commands (e.g. light, light) don't get
    // deduped by ESPHome's text entity. Unused for ha_rest / spi.
    nonce: AtomicU64,
}

impl Daemon {
    fn new(config: Config) -> Self {
        let persisted = load_state();
        info!("Loaded persisted state");
        let mut state = SharedState::new();
        state.fan_states = persisted.fans;
        state.light_states = persisted.lights;
        Self {
            config,
            client: Client::new(),
            state: Mutex::new(state),
            homekit: None,
            radio: None,
            nonce: AtomicU64::new(0),
        }
    }

    /// Send an RF command. Dispatches per `rf.transport` to HA's REST API
    /// (HA → ESPHome → CC1101), the ESPHome web_server (direct HTTP →
    /// ESPHome → CC1101), or a locally-attached CC1101 over SPI.
    async fn send_rf(&self, fan_id: &str, cmd: &str) {
        let cmdid = match CMDS.iter().find(|(k, _)| *k == cmd) {
            Some((_, v)) => *v,
            None => { error!("Unknown RF command: {}", cmd); return; }
        };
        assert_eq!(fan_id.len(), 14, "FAN_ID must be 14 bits, got {}", fan_id.len());
        let bits = format!("1111{}{}", fan_id, cmdid);
        debug_assert_eq!(bits.len(), 25);

        info!(
            "RF → {} {} (bits={}, repeat={}, transport={:?})",
            fan_id, cmd, bits, self.config.rf.repeat, self.config.rf.transport,
        );

        match self.config.rf.transport {
            Transport::HaRest => self.send_rf_ha_rest(fan_id, cmd, &bits).await,
            Transport::Http   => self.send_rf_http(fan_id, cmd, &bits).await,
            Transport::Spi    => self.send_rf_spi(fan_id, cmd, bits).await,
        }
    }

    async fn send_rf_ha_rest(&self, fan_id: &str, cmd: &str, bits: &str) {
        let esp = match &self.config.esphome {
            Some(e) => e,
            None => {
                error!("rf.transport=ha_rest but no esphome: block");
                return;
            }
        };
        let ha = &self.config.homeassistant;
        let endpoint = format!(
            "{}/api/services/esphome/{}_transmit_fan_bits",
            ha.url, esp.device,
        );
        match self.client.post(&endpoint)
            .header("Authorization", format!("Bearer {}", ha.token))
            .json(&json!({ "bits": bits, "repeat": self.config.rf.repeat }))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => error!("RF {} {} → HTTP {}", fan_id, cmd, r.status()),
            Err(e) => error!("RF {} {} failed: {}", fan_id, cmd, e),
        }
    }

    async fn send_rf_http(&self, fan_id: &str, cmd: &str, bits: &str) {
        let http = match self.config.esphome.as_ref().and_then(|e| e.http.as_ref()) {
            Some(h) => h,
            None => {
                error!("rf.transport=http but no esphome.http: block");
                return;
            }
        };
        // <bits>:<repeat>:<nonce> — see the on_value lambda in fan-remote.yaml.
        // The third field is consumed by atoi() up to the next non-digit, so
        // we can chain values; we only need a unique tail to force re-fire.
        // Modulo keeps the URL short (5 digits max) — long URLs have
        // tripped ESPHome's IDF web_server in testing.
        let nonce = self.nonce.fetch_add(1, Ordering::Relaxed) % 100_000;
        let value = format!("{}:{}:{}", bits, self.config.rf.repeat, nonce);
        let url = format!("http://{}:{}/text/rf_tx/set", http.host, http.port);

        let mut req = self.client.post(&url)
            .query(&[("value", value.as_str())])
            // ESPHome's IDF web_server needs Content-Length AND a
            // Content-Type for POSTs — without the latter, query-string
            // parameters can be ignored and the TextCall arrives empty.
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(Duration::from_secs(5));
        if let Some(user) = &http.username {
            req = req.basic_auth(user, http.password.as_deref());
        }

        match req.send().await {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => error!("RF {} {} → HTTP {}", fan_id, cmd, r.status()),
            Err(e) => error!("RF {} {} failed: {}", fan_id, cmd, e),
        }
    }

    #[cfg(target_os = "linux")]
    async fn send_rf_spi(&self, fan_id: &str, cmd: &str, bits: String) {
        let radio = match self.radio.as_ref() {
            Some(r) => Arc::clone(r),
            None => {
                error!("rf.transport=spi but radio not initialised");
                return;
            }
        };
        let repeat = self.config.rf.repeat;
        let fan_id = fan_id.to_string();
        let cmd = cmd.to_string();
        // spidev calls are blocking — hop to the blocking pool so the runtime
        // keeps serving WS / HTTP / HomeKit work while a frame is on the air
        // (~150 ms for 4 retransmits of a 25-bit frame at 3000 baud).
        let res = tokio::task::spawn_blocking(move || {
            let mut guard = radio.lock().expect("CC1101 mutex poisoned");
            guard.transmit(&bits, repeat)
        })
        .await;
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!("RF {} {} → SPI failed: {}", fan_id, cmd, e),
            Err(e) => error!("RF {} {} → SPI join failed: {}", fan_id, cmd, e),
        }
    }

    #[cfg(not(target_os = "linux"))]
    async fn send_rf_spi(&self, fan_id: &str, cmd: &str, _bits: String) {
        error!("RF {} {} → SPI transport not supported on this OS (Linux only)", fan_id, cmd);
    }

    async fn sync_fan_to_ha(&self, room: &str, st: &FanState) {
        if !self.config.homeassistant.listen { return; }
        let ha = &self.config.homeassistant;
        ha_set_state(
            &self.client,
            &ha.url,
            &ha.token,
            &format!("fan.{}_fan", room),
            &st.state.to_lowercase(),
            json!({
                "percentage": st.percentage,
                "direction": st.direction,
                "supported_features": 5,
                "friendly_name": format!("{} Fan", title_case(room)),
            }),
        ).await;
    }

    async fn sync_light_to_ha(&self, room: &str, st: &LightState) {
        if !self.config.homeassistant.listen { return; }
        let ha = &self.config.homeassistant;
        ha_set_state(
            &self.client,
            &ha.url,
            &ha.token,
            &format!("light.{}_fan_light", room),
            &st.state.to_lowercase(),
            json!({ "friendly_name": format!("{} Fan Light", title_case(room)) }),
        ).await;
    }

    async fn init_entities(&self) {
        if !self.config.homeassistant.listen {
            info!("homeassistant.listen=false — skipping entity init");
            return;
        }
        info!("Initialising entities in Home Assistant...");
        for room in self.config.fans.keys() {
            let (fan_st, light_st) = {
                let s = self.state.lock().await;
                (s.fan_states.get(room).cloned().unwrap_or_default(),
                 s.light_states.get(room).cloned().unwrap_or_default())
            };
            self.sync_fan_to_ha(room, &fan_st).await;
            self.sync_light_to_ha(room, &light_st).await;
        }
        info!("Entities initialised");
    }

    async fn set_fan(&self, room: &str, new: FanState) {
        {
            let mut s = self.state.lock().await;
            s.fan_states.insert(room.to_string(), new.clone());
            save_state(&s);
        }
        self.sync_fan_to_ha(room, &new).await;
        if let Some(hk) = &self.homekit {
            hk.update_fan(
                room,
                new.state == "ON",
                new.percentage.min(100) as u8,
                new.direction != "reverse",
            ).await;
        }
    }

    async fn set_light(&self, room: &str, new: LightState) {
        {
            let mut s = self.state.lock().await;
            s.light_states.insert(room.to_string(), new.clone());
            save_state(&s);
        }
        self.sync_light_to_ha(room, &new).await;
        if let Some(hk) = &self.homekit {
            hk.update_light(room, new.state == "ON").await;
        }
    }

    // Maps HA percentage → fan speed index (0=fastest/speed1, 5=slowest/speed6, 6=stop).
    // Inverted because HA 100% = max speed, but speed1 is physically the fastest.
    fn speed_snap(percentage: u32) -> u32 {
        ((100u32.saturating_sub(percentage) as f64 / 100.0) * 6.0).round() as u32
    }

    async fn apply_fan_speed(&self, room: &str, percentage: u32) {
        let fan_id = match self.config.fans.get(room) {
            Some(id) => id.clone(),
            None => { error!("Unknown room: {}", room); return; }
        };
        let direction = {
            let s = self.state.lock().await;
            s.fan_states.get(room).map(|f| f.direction.clone()).unwrap_or_else(|| "forward".into())
        };
        let snapped = Self::speed_snap(percentage);
        if snapped >= 6 {
            self.send_rf(&fan_id, "stop").await;
            self.set_fan(room, FanState { state: "OFF".into(), percentage: 0, direction }).await;
        } else {
            let speed = format!("speed{}", snapped + 1);
            let snapped_pct = (100.0 - snapped as f64 * 100.0 / 6.0).round() as u32;
            info!("{} → {} ({}%)", room, speed, snapped_pct);
            self.send_rf(&fan_id, &speed).await;
            self.set_fan(room, FanState { state: "ON".into(), percentage: snapped_pct, direction }).await;
        }
    }

    async fn schedule_speed(self: Arc<Self>, room: String, percentage: u32) {
        // Immediate optimistic feedback to HA; RF transmission is debounced 1.5 s.
        let snapped = Self::speed_snap(percentage);
        let direction = {
            let s = self.state.lock().await;
            s.fan_states.get(&room).map(|f| f.direction.clone()).unwrap_or_else(|| "forward".into())
        };
        let imm = if snapped >= 6 {
            FanState { state: "OFF".into(), percentage: 0, direction }
        } else {
            let pct = (100.0 - snapped as f64 * 100.0 / 6.0).round() as u32;
            FanState { state: "ON".into(), percentage: pct, direction }
        };
        self.set_fan(&room, imm).await;

        {
            let mut s = self.state.lock().await;
            if let Some(h) = s.pending_timers.remove(&room) {
                h.abort();
            }
        }

        let d = Arc::clone(&self);
        let r = room.clone();
        let handle = tokio::spawn(async move {
            sleep(Duration::from_millis(1500)).await;
            d.apply_fan_speed(&r, percentage).await;
        });
        self.state.lock().await.pending_timers.insert(room, handle.abort_handle());
    }

    async fn handle_service(
        self: Arc<Self>,
        domain: String,
        service: String,
        entity_ids: Vec<String>,
        service_data: Value,
    ) {
        for entity_id in &entity_ids {
            if domain == "fan" {
                let room = match entity_id
                    .strip_prefix("fan.")
                    .and_then(|s| s.strip_suffix("_fan"))
                {
                    Some(r) if self.config.fans.contains_key(r) => r.to_string(),
                    _ => continue,
                };
                info!("fan {} {} {:?}", room, service, service_data);
                match service.as_str() {
                    "turn_on" => {
                        let pct = service_data["percentage"].as_u64().unwrap_or(35) as u32;
                        Arc::clone(&self).schedule_speed(room, pct).await;
                    }
                    "turn_off" => {
                        Arc::clone(&self).schedule_speed(room, 0).await;
                    }
                    "set_percentage" => {
                        let pct = service_data["percentage"].as_u64().unwrap_or(0) as u32;
                        Arc::clone(&self).schedule_speed(room, pct).await;
                    }
                    "set_direction" => {
                        if let Some(fan_id) = self.config.fans.get(&room).cloned() {
                            self.send_rf(&fan_id, "reverse").await;
                        }
                        let dir = service_data["direction"]
                            .as_str()
                            .unwrap_or("forward")
                            .to_string();
                        let current = self.state.lock().await
                            .fan_states.get(&room).cloned().unwrap_or_default();
                        self.set_fan(&room, FanState { direction: dir, ..current }).await;
                    }
                    _ => {}
                }
            } else if domain == "light" {
                let room = match entity_id
                    .strip_prefix("light.")
                    .and_then(|s| s.strip_suffix("_fan_light"))
                {
                    Some(r) if self.config.fans.contains_key(r) => r.to_string(),
                    _ => continue,
                };
                info!("light {} {}", room, service);
                if matches!(service.as_str(), "turn_on" | "turn_off" | "toggle") {
                    if let Some(fan_id) = self.config.fans.get(&room).cloned() {
                        self.send_rf(&fan_id, "light").await;
                    }
                    let current_on = self.state.lock().await
                        .light_states.get(&room)
                        .map(|l| l.state == "ON")
                        .unwrap_or(false);
                    let new_state = match service.as_str() {
                        "turn_on" => "ON",
                        "turn_off" => "OFF",
                        _ => if current_on { "OFF" } else { "ON" },
                    };
                    self.set_light(&room, LightState { state: new_state.into() }).await;
                }
            }
        }
    }

    async fn run(self: Arc<Self>) {
        if !self.config.homeassistant.listen {
            info!("homeassistant.listen=false — daemon idle (HomeKit-only, RF passthrough via REST)");
            std::future::pending::<()>().await;
            return;
        }
        let ha = &self.config.homeassistant;
        let ws_url = ha.url
            .replace("https://", "wss://")
            .replace("http://", "ws://")
            + "/api/websocket";
        info!("Connecting to {}", ws_url);

        loop {
            match connect_async(&ws_url).await {
                Ok((stream, _)) => {
                    let (mut tx, mut rx) = stream.split();
                    let mut msg_id = 1u64;

                    while let Some(item) = rx.next().await {
                        match item {
                            Ok(Message::Text(txt)) => {
                                let data: Value = match serde_json::from_str(&txt) {
                                    Ok(v) => v,
                                    Err(e) => { warn!("JSON parse: {}", e); continue; }
                                };
                                match data["type"].as_str() {
                                    Some("auth_required") => {
                                        let _ = tx.send(Message::Text(json!({
                                            "type": "auth",
                                            "access_token": ha.token,
                                        }).to_string())).await;
                                    }
                                    Some("auth_ok") => {
                                        info!("Authenticated with Home Assistant WebSocket");
                                        msg_id += 1;
                                        let _ = tx.send(Message::Text(json!({
                                            "id": msg_id,
                                            "type": "subscribe_events",
                                            "event_type": "call_service",
                                        }).to_string())).await;
                                    }
                                    Some("auth_invalid") => {
                                        error!("Authentication failed — check your token");
                                        break;
                                    }
                                    Some("event") => {
                                        let ed = &data["event"]["data"];
                                        let domain = ed["domain"].as_str().unwrap_or("").to_string();
                                        let service = ed["service"].as_str().unwrap_or("").to_string();
                                        let sd = ed.get("service_data").cloned().unwrap_or(json!({}));

                                        // HA 2022+ uses target.entity_id; older uses service_data.entity_id
                                        let raw_ids = ed.get("target")
                                            .and_then(|t| t.get("entity_id"))
                                            .or_else(|| sd.get("entity_id"));
                                        let entity_ids: Vec<String> = match raw_ids {
                                            Some(Value::String(s)) => vec![s.clone()],
                                            Some(Value::Array(a)) => a.iter()
                                                .filter_map(|v| v.as_str().map(String::from))
                                                .collect(),
                                            _ => vec![],
                                        };

                                        if (domain == "fan" || domain == "light") && !entity_ids.is_empty() {
                                            let d = Arc::clone(&self);
                                            tokio::spawn(async move {
                                                d.handle_service(domain, service, entity_ids, sd).await;
                                            });
                                        }
                                    }
                                    Some("result") if data["success"] == false => {
                                        warn!("WS command failed: {}", data);
                                    }
                                    _ => {}
                                }
                            }
                            Ok(Message::Close(_)) => break,
                            Err(e) => { error!("WS error: {}", e); break; }
                            _ => {}
                        }
                    }
                }
                Err(e) => error!("WS connect failed: {}", e),
            }
            info!("Reconnecting in 10 s...");
            sleep(Duration::from_secs(10)).await;
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            // Default to INFO across the board. hap=debug used to be on by default
            // but hap::transport::tcp logs every byte written, which fills the
            // journal once a HomeKit controller is active. Override per-run via
            // RUST_LOG, e.g. `RUST_LOG=info,hap=debug` for protocol tracing.
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "config.yaml".into());
    let raw = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|e| panic!("Cannot read {}: {}", config_path, e));
    let config: Config = serde_yaml::from_str(&raw)
        .unwrap_or_else(|e| panic!("Cannot parse {}: {}", config_path, e));

    // Transport ↔ config-block consistency. We validate up-front rather than
    // erroring per-frame so a misconfigured deployment fails loudly at boot.
    match config.rf.transport {
        Transport::HaRest => {
            if config.esphome.is_none() {
                panic!("rf.transport=ha_rest requires an esphome: {{ device, ... }} block");
            }
        }
        Transport::Http => {
            let http_ok = config.esphome.as_ref()
                .and_then(|e| e.http.as_ref())
                .is_some();
            if !http_ok {
                panic!("rf.transport=http requires an esphome.http: {{ host, ... }} block");
            }
        }
        Transport::Spi => {
            if config.cc1101.is_none() {
                panic!("rf.transport=spi requires a cc1101: {{ device, ... }} block");
            }
            #[cfg(not(target_os = "linux"))]
            panic!("rf.transport=spi is Linux-only (spidev unavailable on this OS)");
        }
    }

    let mut daemon = Daemon::new(config);

    // Bring up the locally-attached CC1101 if we're driving it directly.
    #[cfg(target_os = "linux")]
    if daemon.config.rf.transport == Transport::Spi {
        let cfg = daemon.config.cc1101.as_ref().expect("validated above");
        info!(
            "Initialising CC1101 on {} @ {} Hz, PA=0x{:02X}",
            cfg.device, cfg.frequency_hz, cfg.power,
        );
        let mut radio = cc1101::Cc1101::open(&cfg.device)
            .unwrap_or_else(|e| panic!("Cannot open {}: {}", cfg.device, e));
        radio.init(cfg.frequency_hz, cfg.power)
            .unwrap_or_else(|e| panic!("CC1101 init failed: {}", e));
        daemon.radio = Some(Arc::new(std::sync::Mutex::new(radio)));
        info!("CC1101 ready");
    }

    // Optional HomeKit bridge
    if let Some(hk_cfg) = daemon.config.homekit.as_ref().filter(|c| c.enabled) {
        let rooms: Vec<String> = daemon.config.fans.keys().cloned().collect();
        let pin_bytes = parse_pin(&hk_cfg.pin);
        let bridge_name = hk_cfg.name.clone().unwrap_or_else(hostname);
        let bind_ip: std::net::IpAddr = hk_cfg.bind.parse()
            .unwrap_or_else(|e| panic!("invalid homekit.bind \"{}\": {}", hk_cfg.bind, e));
        match homekit::HomeKit::new(&bridge_name, &rooms, bind_ip, hk_cfg.port, &hk_cfg.persist_dir, pin_bytes).await {
            Ok((hk, server)) => {
                info!("HomeKit bridge \"{}\" on {}:{} (persist={})", bridge_name, bind_ip, hk_cfg.port, hk_cfg.persist_dir);
                daemon.homekit = Some(hk);
                tokio::spawn(homekit::run_server(server));
            }
            Err(e) => error!("HomeKit bridge failed to start: {}", e),
        }
    }

    let daemon = Arc::new(daemon);

    // If HomeKit is enabled, consume its command stream and dispatch as service calls.
    if let Some(hk) = daemon.homekit.clone() {
        let d = Arc::clone(&daemon);
        tokio::spawn(async move {
            let mut rx = match hk.cmd_rx.lock().await.take() {
                Some(r) => r,
                None => return,
            };
            while let Some(cmd) = rx.recv().await {
                info!("HomeKit → {} {} {} {}", cmd.domain, cmd.service, cmd.room, cmd.data);
                let entity = match cmd.domain {
                    "fan" => format!("fan.{}_fan", cmd.room),
                    "light" => format!("light.{}_fan_light", cmd.room),
                    _ => continue,
                };
                Arc::clone(&d).handle_service(
                    cmd.domain.to_string(),
                    cmd.service.to_string(),
                    vec![entity],
                    cmd.data,
                ).await;
            }
        });
    }

    daemon.init_entities().await;
    Arc::clone(&daemon).run().await;
}
