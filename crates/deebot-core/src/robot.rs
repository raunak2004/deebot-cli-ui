//! Everything the robot understands, as plain data that a CLI or UI can display.
//!
//! Request formats were confirmed against a real T50; where the robot's reply shape is unsure,
//! readings keep the raw JSON so callers can show what the robot actually sent.

use std::fmt;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::ecovacs::Device;
use crate::session::Login;

/// What the robot should do with the current clean.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum CleanAction {
    /// Start an automatic whole-home clean (resumes instead if the robot is paused)
    Start,
    Pause,
    /// Continue a paused clean (starts a new one if the robot isn't paused)
    Resume,
    /// End the current clean; the robot stays where it is
    Stop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum FanLevel {
    Quiet,
    Normal,
    Max,
    /// The strongest suction (the robot's "max+")
    MaxPlus,
}

impl FanLevel {
    pub const ALL: [FanLevel; 4] = [Self::Quiet, Self::Normal, Self::Max, Self::MaxPlus];

    /// The numeric level the robot uses (quiet is 1000, the rest count up from 0).
    pub fn value(self) -> i64 {
        match self {
            Self::Quiet => 1000,
            Self::Normal => 0,
            Self::Max => 1,
            Self::MaxPlus => 2,
        }
    }
}

pub fn fan_speed_name(speed: i64) -> &'static str {
    match speed {
        1000 => "quiet",
        0 => "normal",
        1 => "max",
        2 => "max+",
        _ => "unknown",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum WorkMode {
    VacuumAndMop,
    Vacuum,
    Mop,
    MopAfterVacuum,
}

impl WorkMode {
    pub const ALL: [WorkMode; 4] = [Self::VacuumAndMop, Self::Vacuum, Self::Mop, Self::MopAfterVacuum];

    pub fn value(self) -> i64 {
        match self {
            Self::VacuumAndMop => 0,
            Self::Vacuum => 1,
            Self::Mop => 2,
            Self::MopAfterVacuum => 3,
        }
    }
}

pub fn work_mode_name(value: i64) -> &'static str {
    match value {
        0 => "vacuum and mop",
        1 => "vacuum",
        2 => "mop",
        3 => "mop after vacuum",
        _ => "unknown",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum EmptyFrequency {
    Auto,
    Smart,
}

impl EmptyFrequency {
    pub fn value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Smart => "smart",
        }
    }
}

/// An on/off robot setting: its get and set commands and the key holding its value.
pub struct Toggle {
    pub label: &'static str,
    get: &'static str,
    set: &'static str,
    key: &'static str,
    /// Other fields the robot wants echoed back unchanged when setting (e.g. a level)
    keep: &'static [&'static str],
}

pub const CHILD_LOCK: Toggle = Toggle { label: "Child lock", get: "getChildLock", set: "setChildLock", key: "on", keep: &[] };
// Ecovacs spells this command "Carpert"
pub const CARPET_BOOST: Toggle = Toggle { label: "Carpet boost", get: "getCarpertPressure", set: "setCarpertPressure", key: "enable", keep: &[] };
pub const ADVANCED_MODE: Toggle = Toggle { label: "Advanced mode", get: "getAdvancedMode", set: "setAdvancedMode", key: "enable", keep: &[] };
pub const SWEEP_MODE: Toggle = Toggle { label: "Sweep mode", get: "getSweepMode", set: "setSweepMode", key: "type", keep: &[] };
pub const TRUE_DETECT: Toggle = Toggle { label: "True detect", get: "getTrueDetect", set: "setTrueDetect", key: "enable", keep: &["level"] };
pub const VOICE_ASSISTANT: Toggle = Toggle { label: "Voice assistant", get: "getVoiceAssistantState", set: "setVoiceAssistantState", key: "enable", keep: &[] };

/// Every on/off switch the robot exposes.
pub const ALL_TOGGLES: [&Toggle; 6] = [&CHILD_LOCK, &CARPET_BOOST, &ADVANCED_MODE, &SWEEP_MODE, &TRUE_DETECT, &VOICE_ASSISTANT];

impl Toggle {
    /// The robot command that reads this setting (shown by debugging output).
    pub fn get_command(&self) -> &'static str {
        self.get
    }
}

/// The extra on/off switches, selectable by name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum SwitchName {
    AdvancedMode,
    SweepMode,
    TrueDetect,
    VoiceAssistant,
}

impl SwitchName {
    pub fn toggle(self) -> &'static Toggle {
        match self {
            Self::AdvancedMode => &ADVANCED_MODE,
            Self::SweepMode => &SWEEP_MODE,
            Self::TrueDetect => &TRUE_DETECT,
            Self::VoiceAssistant => &VOICE_ASSISTANT,
        }
    }
}

/// The state of an on/off setting as the robot reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToggleState {
    On,
    Off,
    /// An unexpected reply, kept as the robot sent it
    Unknown(String),
}

impl fmt::Display for ToggleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::On => f.write_str("on"),
            Self::Off => f.write_str("off"),
            Self::Unknown(data) => write!(f, "unknown ({data})"),
        }
    }
}

/// A numeric reading; `raw` is the robot's reply for when `value` couldn't be read.
#[derive(Clone, Debug)]
pub struct Reading {
    pub value: Option<i64>,
    pub raw: String,
}

/// The `freeClean` value for cleaning these rooms: `1,3;1,4` for rooms 3 and 4.
pub fn room_clean_value(room_ids: &[i64]) -> String {
    room_ids.iter().map(|id| format!("1,{id}")).collect::<Vec<_>>().join(";")
}

fn reading(data: &Value, key: &str) -> Reading {
    Reading { value: data.get(key).and_then(Value::as_i64), raw: data.to_string() }
}

/// One line of the status report, ready to display.
#[derive(Clone, Debug)]
pub struct StatusLine {
    pub label: &'static str,
    pub command: &'static str,
    pub text: String,
    pub raw: String,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub name: String,
    pub class: String,
    pub battery: Option<i64>,
    pub docked: Option<bool>,
    pub robot_state: Option<String>,
    pub station_state: Option<String>,
    pub paused: bool,
    pub lines: Vec<StatusLine>,
}

#[derive(Clone, Debug)]
pub struct Stats {
    pub total_cleans: String,
    pub total_area: String,
    pub total_time: String,
    pub last_area: String,
    pub last_time: String,
}

/// How worn one part is. Units of `left` and `total` aren't documented, so both are kept as sent.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub left: Option<i64>,
    pub total: Option<i64>,
    pub raw: String,
}

impl Part {
    pub fn percent(&self) -> Option<i64> {
        match (self.left, self.total) {
            (Some(left), Some(total)) if total > 0 => Some(left * 100 / total),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Volume {
    pub current: Option<i64>,
    pub total: Option<i64>,
    pub raw: String,
}

#[derive(Clone, Debug)]
pub struct Network {
    pub ssid: String,
    pub ip: String,
    pub rssi: String,
    pub mac: String,
}

#[derive(Clone, Debug)]
pub struct Firmware {
    pub firmware: String,
    pub hardware: String,
    pub software: String,
}

#[derive(Clone, Debug)]
pub struct AutoEmpty {
    pub enabled: Option<bool>,
    pub frequency: Option<String>,
    pub raw: String,
}

#[derive(Clone, Debug)]
pub struct Room {
    pub id: i64,
    pub name: String,
}

impl Room {
    pub fn label(&self) -> &str {
        if self.name.is_empty() { "(unnamed)" } else { &self.name }
    }
}

/// A spot on the map, in the robot's own units, with its heading.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
}

/// Where the robot and its dock are.
#[derive(Clone, Copy, Debug, Default)]
pub struct Positions {
    pub robot: Option<Pose>,
    pub charger: Option<Pose>,
}

/// The floor plan, plus how the Ecovacs app turns it on screen.
#[derive(Clone, Debug)]
pub struct MapView {
    pub map: crate::map::MapData,
    /// Rotation in degrees: 0, 90, 180 or 270
    pub angle: i64,
}

/// The wear parts the T50 reports, as (id sent to the robot, display name).
const LIFESPAN_PARTS: [(&str, &str); 6] = [
    ("brush", "Main brush"),
    ("heap", "Filter"),
    ("handFilter", "Hand filter"),
    ("sideBrush", "Side brush"),
    ("cleaningSolution", "Cleaning solution"),
    ("roundMop", "Mop pads"),
];

/// A robot on a logged-in account.
pub struct Robot {
    login: Arc<Login>,
    device: Device,
}

impl Robot {
    pub fn new(login: Arc<Login>, device: Device) -> Self {
        Self { login, device }
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The robot's nickname, or its name when it has none.
    pub fn name(&self) -> &str {
        self.device.nick.as_deref().filter(|n| !n.is_empty()).unwrap_or(&self.device.name)
    }

    /// Send a command and return the robot's full reply.
    async fn send(&self, name: &str, args: Option<Value>) -> Result<Value> {
        self.login.client.send_command(&self.login.credentials, &self.device, name, args).await
    }

    /// Send a command and fail if the robot answers with a non-zero code.
    async fn send_checked(&self, name: &str, args: Value) -> Result<()> {
        let resp = self.send(name, Some(args)).await?;
        let code = resp.pointer("/body/code").and_then(Value::as_i64);
        if code.is_some_and(|code| code != 0) {
            bail!("the robot rejected {name}: {resp}");
        }
        Ok(())
    }

    /// Ask the robot for a value; returns the `data` object of its answer.
    async fn query(&self, name: &str) -> Result<Value> {
        self.query_args(name, None).await
    }

    /// Like [`Self::query`], for the few readings that need arguments.
    async fn query_args(&self, name: &str, args: Option<Value>) -> Result<Value> {
        let resp = self.send(name, args).await?;
        if let Some(code) = resp.pointer("/body/code").and_then(Value::as_i64).filter(|code| *code != 0) {
            bail!("{name} returned code {code}");
        }
        Ok(resp.pointer("/body/data").cloned().unwrap_or(Value::Null))
    }

    /// Send any command and return the robot's full reply. For exploring what a robot supports;
    /// nothing here checks that the command is safe.
    pub async fn raw(&self, name: &str, args: Option<Value>) -> Result<Value> {
        self.send(name, args).await
    }

    // --- actions ---

    pub async fn locate(&self) -> Result<()> {
        self.send_checked("playSound", json!({ "count": 1, "sid": 30 })).await
    }

    pub async fn dock(&self) -> Result<()> {
        self.send_checked("charge", json!({ "act": "go" })).await
    }

    /// Returns what was done, e.g. "started cleaning".
    pub async fn clean(&self, action: CleanAction) -> Result<&'static str> {
        // The robot wants "resume" only while paused and "start" otherwise
        let paused = if matches!(action, CleanAction::Start | CleanAction::Resume) {
            let state = self.query("getWorkState").await?;
            state.get("paused").and_then(Value::as_i64) == Some(1)
        } else {
            false
        };
        let (act, content, done) = match (action, paused) {
            (CleanAction::Start, false) | (CleanAction::Resume, false) => ("start", json!({ "type": "auto" }), "started cleaning"),
            (CleanAction::Start, true) | (CleanAction::Resume, true) => ("resume", json!({}), "resumed cleaning"),
            (CleanAction::Pause, _) => ("pause", json!({ "type": "" }), "paused"),
            (CleanAction::Stop, _) => ("stop", json!({ "type": "" }), "stopped cleaning"),
        };
        self.send_checked("clean_V2", json!({ "act": act, "content": content })).await?;
        Ok(done)
    }

    /// Clean one or more rooms. This robot has no "spotArea" request: it takes a `freeClean` whose
    /// value is one `1,<room id>` per room joined by `;` (the 1 is the number of passes, per the
    /// open-source clients; confirmed by trying values and watching where the robot went).
    /// Returns the value that was sent.
    ///
    /// The robot falls back to vacuum-and-mop after a stop or a trip to the dock, so pass `mode` to
    /// set what it cleans with right before starting.
    pub async fn clean_rooms(&self, room_ids: &[i64], mode: Option<WorkMode>) -> Result<String> {
        if room_ids.is_empty() {
            bail!("no rooms given");
        }
        let known = self.rooms().await?;
        for id in room_ids {
            if !known.iter().any(|room| room.id == *id) {
                bail!("room {id} isn't on the active map");
            }
        }
        if let Some(mode) = mode {
            self.set_work_mode(mode).await?;
        }
        let value = room_clean_value(room_ids);
        self.send_checked("clean_V2", json!({ "act": "start", "content": { "type": "freeClean", "value": value } })).await?;
        Ok(value)
    }

    pub async fn empty_dustbin(&self) -> Result<()> {
        // Emptying only makes sense on the dock, so check that before asking the dock to run
        let charge = self.query("getChargeState").await?;
        if charge.get("isCharging").and_then(Value::as_i64) != Some(1) {
            bail!("the robot isn't on its dock; send it home with `dock` first");
        }
        // station action 1 is "empty dustbin"
        self.send_checked("stationAction", json!({ "act": 1, "type": 1 })).await
    }

    // --- readings ---

    pub async fn status(&self) -> Result<Status> {
        // The battery is queried first: it fails fast with a clear message when the robot is offline
        let battery = self.query("getBattery").await?;
        let queries = [
            ("getChargeState", "Charging"),
            ("getWorkState", "Work state"),
            ("getSpeed", "Suction"),
            ("getWaterInfo", "Mop water"),
            ("getError", "Errors"),
        ];
        let mut results = vec![("getBattery", "Battery", Ok(battery))];
        for (name, label) in queries {
            results.push((name, label, self.query(name).await));
        }

        let mut status = Status {
            name: self.name().to_string(),
            class: self.device.class.clone(),
            battery: None,
            docked: None,
            robot_state: None,
            station_state: None,
            paused: false,
            lines: Vec::new(),
        };
        for (command, label, result) in results {
            let raw = result.as_ref().map(Value::to_string).unwrap_or_default();
            let text = match result {
                Err(err) => format!("unavailable ({err:#})"),
                Ok(data) => describe_status(command, &data, &mut status),
            };
            status.lines.push(StatusLine { label, command, text, raw });
        }
        Ok(status)
    }

    pub async fn stats(&self) -> Result<Stats> {
        // The units of area and time aren't documented, so the numbers are shown as the robot sends them
        let total = self.query("getTotalStats").await?;
        let last = self.query("getStats").await?;
        let num = |data: &Value, key: &str| data.get(key).map_or_else(|| "?".to_string(), Value::to_string);
        Ok(Stats {
            total_cleans: num(&total, "count"),
            total_area: num(&total, "area"),
            total_time: num(&total, "time"),
            last_area: num(&last, "area"),
            last_time: num(&last, "time"),
        })
    }

    pub async fn lifespan(&self) -> Result<Vec<Part>> {
        let ids: Vec<&str> = LIFESPAN_PARTS.iter().map(|(id, _)| *id).collect();
        let resp = self.send("getLifeSpan", Some(json!(ids))).await?;
        let items = resp.pointer("/body/data").and_then(Value::as_array).context("unexpected getLifeSpan response")?;
        Ok(items
            .iter()
            .map(|item| {
                let id = item.get("type").and_then(Value::as_str).unwrap_or("?");
                let name = LIFESPAN_PARTS.iter().find(|(part, _)| *part == id).map_or(id, |(_, name)| *name);
                Part {
                    name: name.to_string(),
                    left: item.get("left").and_then(Value::as_i64),
                    total: item.get("total").and_then(Value::as_i64),
                    raw: item.to_string(),
                }
            })
            .collect())
    }

    pub async fn network(&self) -> Result<Network> {
        let data = self.query("getNetInfo").await?;
        let text = |key: &str| data.get(key).map_or_else(|| "?".to_string(), |v| v.as_str().map_or_else(|| v.to_string(), str::to_string));
        Ok(Network { ssid: text("ssid"), ip: text("ip"), rssi: text("rssi"), mac: text("mac") })
    }

    /// Versions come back in the header of every reply, so any cheap query will do.
    pub async fn firmware(&self) -> Result<Firmware> {
        let resp = self.send("getBattery", None).await?;
        let text = |key: &str| resp.pointer(&format!("/header/{key}")).and_then(Value::as_str).unwrap_or("unknown").to_string();
        Ok(Firmware { firmware: text("fwVer"), hardware: text("hwVer"), software: text("wkVer") })
    }

    // --- settings ---

    pub async fn toggle(&self, toggle: &Toggle) -> Result<ToggleState> {
        let data = self.query(toggle.get).await?;
        Ok(match data.get(toggle.key).and_then(Value::as_i64) {
            Some(1) => ToggleState::On,
            Some(0) => ToggleState::Off,
            // Unexpected shape: keep what the robot sent instead of guessing
            _ => ToggleState::Unknown(data.to_string()),
        })
    }

    /// Switch a setting and return its state as read back from the robot.
    pub async fn set_toggle(&self, toggle: &Toggle, on: bool) -> Result<ToggleState> {
        let mut args = serde_json::Map::new();
        args.insert(toggle.key.to_string(), json!(i64::from(on)));
        if !toggle.keep.is_empty() {
            let current = self.query(toggle.get).await?;
            for field in toggle.keep {
                if let Some(value) = current.get(*field) {
                    args.insert((*field).to_string(), value.clone());
                }
            }
        }
        self.send_checked(toggle.set, Value::Object(args)).await?;
        self.toggle(toggle).await
    }

    /// The raw reply of a switch's get command (for debugging).
    pub async fn toggle_raw(&self, toggle: &Toggle) -> Result<String> {
        Ok(self.query(toggle.get).await?.to_string())
    }

    pub async fn efficiency(&self) -> Result<Reading> {
        // What each number means isn't confirmed, so the raw value is shown
        Ok(reading(&self.query("getEfficiency").await?, "efficiency"))
    }

    pub async fn fan(&self) -> Result<Reading> {
        Ok(reading(&self.query("getSpeed").await?, "speed"))
    }

    pub async fn set_fan(&self, level: FanLevel) -> Result<()> {
        self.send_checked("setSpeed", json!({ "speed": level.value() })).await
    }

    /// The mop water level, 0 to 50.
    pub async fn water(&self) -> Result<Reading> {
        Ok(reading(&self.query("getWaterInfo").await?, "customAmount"))
    }

    pub async fn set_water(&self, level: u8) -> Result<()> {
        self.send_checked("setWaterInfo", json!({ "customAmount": level })).await
    }

    pub async fn work_mode(&self) -> Result<Reading> {
        Ok(reading(&self.query("getWorkMode").await?, "mode"))
    }

    pub async fn set_work_mode(&self, mode: WorkMode) -> Result<()> {
        self.send_checked("setWorkMode", json!({ "mode": mode.value() })).await
    }

    pub async fn clean_count(&self) -> Result<Reading> {
        Ok(reading(&self.query("getCleanCount").await?, "count"))
    }

    pub async fn set_clean_count(&self, count: u8) -> Result<()> {
        self.send_checked("setCleanCount", json!({ "count": count })).await
    }

    pub async fn volume(&self) -> Result<Volume> {
        let data = self.query("getVolume").await?;
        Ok(Volume {
            current: data.get("volume").and_then(Value::as_i64),
            total: data.get("total").and_then(Value::as_i64),
            raw: data.to_string(),
        })
    }

    pub async fn set_volume(&self, level: i64) -> Result<()> {
        let total = self.volume().await?.total;
        if level < 0 || total.is_some_and(|total| level > total) {
            bail!("volume must be between 0 and {}", total.unwrap_or(10));
        }
        self.send_checked("setVolume", json!({ "volume": level })).await
    }

    pub async fn auto_empty(&self) -> Result<AutoEmpty> {
        let data = self.query("getAutoEmpty").await?;
        Ok(AutoEmpty {
            enabled: match data.get("enable").and_then(Value::as_i64) {
                Some(1) => Some(true),
                Some(0) => Some(false),
                _ => None,
            },
            frequency: data.get("frequency").and_then(Value::as_str).filter(|f| !f.is_empty()).map(str::to_string),
            raw: data.to_string(),
        })
    }

    pub async fn set_auto_empty(&self, state: Option<bool>, frequency: Option<EmptyFrequency>) -> Result<()> {
        let mut args = serde_json::Map::new();
        if let Some(state) = state {
            args.insert("enable".to_string(), json!(i64::from(state)));
        } else {
            // The robot rejects a frequency on its own, so send the current on/off state with it
            let current = self.query("getAutoEmpty").await?;
            if let Some(enable) = current.get("enable") {
                args.insert("enable".to_string(), enable.clone());
            }
        }
        if let Some(frequency) = frequency {
            args.insert("frequency".to_string(), json!(frequency.value()));
        }
        self.send_checked("setAutoEmpty", Value::Object(args)).await
    }

    // --- map ---

    /// The id and rotation of the map the robot is using (it keeps several slots, most of them empty).
    async fn active_map(&self) -> Result<(String, i64)> {
        let data = self.query("getCachedMapInfo").await?;
        let maps = data.get("info").and_then(Value::as_array).context("unexpected getCachedMapInfo response")?;
        let active = maps
            .iter()
            .find(|m| m.get("using").and_then(Value::as_i64) == Some(1))
            .or_else(|| maps.iter().find(|m| m.get("built").and_then(Value::as_i64) == Some(1)))
            .context("the robot has no saved map; run a clean first so it can build one")?;
        let id = active.get("mid").and_then(Value::as_str).map(str::to_string).context("the active map has no id")?;
        Ok((id, active.get("angle").and_then(Value::as_i64).unwrap_or(0)))
    }

    /// The id of the map the robot is using.
    pub async fn active_map_id(&self) -> Result<String> {
        Ok(self.active_map().await?.0)
    }

    /// The floor-plan outline, room shapes and wall lines of the active map.
    pub async fn map_info(&self) -> Result<crate::map::MapData> {
        let mid = self.active_map_id().await?;
        self.map_info_for(&mid).await
    }

    async fn map_info_for(&self, mid: &str) -> Result<crate::map::MapData> {
        let data = self.query_args("getMapInfo_V2", Some(json!({ "mid": mid, "type": "0" }))).await?;
        let encoded = data.get("info").and_then(Value::as_str).context("the robot sent no map outline")?;
        crate::map::decode_map_info(encoded)
    }

    /// The floor plan together with the rotation the Ecovacs app shows it at.
    pub async fn map_view(&self) -> Result<MapView> {
        let (mid, angle) = self.active_map().await?;
        Ok(MapView { map: self.map_info_for(&mid).await?, angle })
    }

    /// The path the robot has cleaned so far, in the robot's own units.
    pub async fn trace(&self) -> Result<Vec<crate::map::TracePoint>> {
        Ok(self.trace_since(0).await?.1)
    }

    /// The path from the page containing point `known` onward, as (index of the first point
    /// returned, points). Callers keep the first `index` points they already have and replace the
    /// rest. If the robot's path is shorter than `known`, a new clean has started, so it restarts.
    pub async fn trace_since(&self, known: usize) -> Result<(usize, Vec<crate::map::TracePoint>)> {
        const PAGE: usize = 200;
        let mut first = known / PAGE * PAGE;
        let mut start = first;
        let mut points = Vec::new();
        // The path arrives 200 points at a time; the cap stops a runaway loop on odd replies
        for _ in 0..60 {
            let data = self.query_args("getMapTrace", Some(json!({ "pointCount": PAGE, "traceStart": start }))).await?;
            let total = data.get("totalCount").and_then(Value::as_u64).unwrap_or(0) as usize;
            if start == first && total < known && first > 0 {
                // A new clean: the old path is gone, so start over from the beginning
                first = 0;
                start = 0;
                points.clear();
                continue;
            }
            let Some(value) = data.get("traceValue").and_then(Value::as_str) else { break };
            points.extend(crate::map::decode_trace(value)?);
            start += PAGE;
            if start >= total {
                break;
            }
        }
        Ok((first, points))
    }

    /// Where the robot and its dock are right now.
    pub async fn positions(&self) -> Result<Positions> {
        let data = self.query_args("getPos", Some(json!(["chargePos", "deebotPos"]))).await?;
        let pose = |value: Option<&Value>| -> Option<Pose> {
            // The robot sends either one position or a list of them
            let entry = match value? {
                Value::Array(list) => list.first()?,
                other => other,
            };
            if entry.get("invalid").and_then(Value::as_i64).unwrap_or(0) != 0 {
                return None;
            }
            Some(Pose {
                x: entry.get("x")?.as_f64()? as f32,
                y: entry.get("y")?.as_f64()? as f32,
                angle: entry.get("a").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            })
        };
        Ok(Positions { robot: pose(data.get("deebotPos")), charger: pose(data.get("chargePos")) })
    }

    /// The rooms of the active map.
    pub async fn rooms(&self) -> Result<Vec<Room>> {
        let mid = self.active_map_id().await?;
        let resp = self.send("getMapSet_V2", Some(json!({ "mid": mid, "type": "ar" }))).await?;
        let encoded = resp.pointer("/body/data/subsets").and_then(Value::as_str).context("the robot sent no room data")?;
        parse_rooms(encoded)
    }
}

/// Turn one status reply into display text, filling in the structured fields on the way.
fn describe_status(command: &str, data: &Value, status: &mut Status) -> String {
    match (command, data) {
        ("getBattery", d) => match d.get("value").and_then(Value::as_i64) {
            Some(v) => {
                status.battery = Some(v);
                format!("{v}%")
            }
            None => d.to_string(),
        },
        ("getChargeState", d) => match d.get("isCharging").and_then(Value::as_i64) {
            Some(1) => {
                status.docked = Some(true);
                "yes (docked)".to_string()
            }
            Some(_) => {
                status.docked = Some(false);
                "no".to_string()
            }
            None => d.to_string(),
        },
        ("getSpeed", d) => d
            .get("speed")
            .and_then(Value::as_i64)
            .map_or_else(|| d.to_string(), |speed| format!("{} ({speed})", fan_speed_name(speed))),
        ("getError", d) => match d.get("code").and_then(Value::as_array) {
            Some(codes) if codes.iter().all(|c| c.as_i64() == Some(0)) => "none".to_string(),
            Some(codes) => format!("codes {}", Value::Array(codes.clone())),
            None => d.to_string(),
        },
        ("getWorkState", d) => {
            let state = |part: &str| d.pointer(&format!("/{part}/state")).and_then(Value::as_str);
            let paused = d.get("paused").and_then(Value::as_i64) == Some(1);
            match (state("robotState"), state("stationState")) {
                (Some(robot), station) => {
                    status.robot_state = Some(robot.to_string());
                    status.station_state = station.map(str::to_string);
                    status.paused = paused;
                    format!("robot {robot}, station {}{}", station.unwrap_or("unknown"), if paused { ", paused" } else { "" })
                }
                _ => d.to_string(),
            }
        }
        ("getWaterInfo", d) => match d.get("customAmount").and_then(Value::as_i64) {
            Some(amount) => format!(
                "level {amount}, mop {}",
                if d.get("enable").and_then(Value::as_i64) == Some(1) { "attached" } else { "not attached" }
            ),
            None => d.to_string(),
        },
        (_, d) => d.to_string(),
    }
}

/// Decode the robot's room list: base64 of a zstd-compressed JSON list of arrays of strings,
/// each with the room id first and its name second.
pub(crate) fn parse_rooms(encoded: &str) -> Result<Vec<Room>> {
    let text = crate::map::unpack_zstd_base64(encoded).context("room data couldn't be decoded")?;

    let entries: Vec<Vec<String>> = serde_json::from_str(&text).context("room data isn't the expected list")?;
    let mut rooms = Vec::new();
    for entry in entries {
        let id = entry.first().and_then(|id| id.parse().ok()).context("a room has no numeric id")?;
        rooms.push(Room { id, name: entry.get(1).cloned().unwrap_or_default() });
    }
    Ok(rooms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn room_clean_value_joins_rooms_with_semicolons() {
        assert_eq!(room_clean_value(&[3]), "1,3");
        assert_eq!(room_clean_value(&[3, 4]), "1,3;1,4");
    }

    fn encode(json: &str) -> String {
        let compressed = ruzstd::encoding::compress_to_vec(json.as_bytes(), ruzstd::encoding::CompressionLevel::Fastest);
        base64::engine::general_purpose::STANDARD.encode(compressed)
    }

    #[test]
    fn decodes_compressed_room_lists() {
        let rooms = parse_rooms(&encode(r#"[["1","Kitchen","0","0","0","1","2","1-1-1","0","0"],["7","","0","0","0","1","2","1-1-1","0","0"]]"#)).unwrap();
        assert_eq!(rooms.len(), 2);
        assert_eq!((rooms[0].id, rooms[0].name.as_str()), (1, "Kitchen"));
        assert_eq!((rooms[1].id, rooms[1].name.as_str()), (7, ""));
        assert_eq!(rooms[1].label(), "(unnamed)");
    }

    #[test]
    fn rejects_data_that_is_not_zstd() {
        assert!(parse_rooms("aGVsbG8=").is_err());
    }

    #[test]
    fn status_text_is_filled_in_from_replies() {
        let mut status = Status {
            name: String::new(),
            class: String::new(),
            battery: None,
            docked: None,
            robot_state: None,
            station_state: None,
            paused: false,
            lines: Vec::new(),
        };
        assert_eq!(describe_status("getBattery", &json!({ "isLow": 0, "value": 87 }), &mut status), "87%");
        assert_eq!(describe_status("getChargeState", &json!({ "isCharging": 1 }), &mut status), "yes (docked)");
        assert_eq!(describe_status("getSpeed", &json!({ "speed": 1 }), &mut status), "max (1)");
        assert_eq!(describe_status("getError", &json!({ "code": [] }), &mut status), "none");
        let work = json!({ "paused": 1, "robotState": { "state": "cleaning" }, "stationState": { "state": "idle" } });
        assert_eq!(describe_status("getWorkState", &work, &mut status), "robot cleaning, station idle, paused");
        assert_eq!(
            (status.battery, status.docked, status.robot_state.as_deref(), status.paused),
            (Some(87), Some(true), Some("cleaning"), true)
        );
    }

    #[test]
    fn part_percent_needs_a_positive_total() {
        let part = |left, total| Part { name: String::new(), left, total, raw: String::new() };
        assert_eq!(part(Some(13580), Some(18000)).percent(), Some(75));
        assert_eq!(part(Some(5), Some(0)).percent(), None);
        assert_eq!(part(None, Some(10)).percent(), None);
    }
}
