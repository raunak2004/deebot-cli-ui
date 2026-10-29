use std::io::{self, Write};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};

use deebot_core::robot::{
    fan_speed_name, work_mode_name, CleanAction, EmptyFrequency, FanLevel, SwitchName, WorkMode, ALL_TOGGLES,
    CARPET_BOOST, CHILD_LOCK,
};
use deebot_core::serde_json::{self, json, Value};
use deebot_core::{session, Login, Robot};

const AFTER_HELP: &str = "\
SESSION:
    After the first login the account token (never the password) is saved to
    ~/.config/deebot-cli/session.json, readable only by you, and reused so later
    commands don't ask for the password. Run `deebot-cli logout` to delete it.

ENVIRONMENT:
    ECOVACS_EMAIL      Account email (or pass --email); defaults to the saved one
    ECOVACS_PASSWORD   Account password; if unset you are prompted without echo
    ECOVACS_COUNTRY    Two-letter country of your Ecovacs account (or pass --country)

EXAMPLES:
    deebot-cli login
    deebot-cli --country IN devices
    deebot-cli status
    deebot-cli locate
    deebot-cli clean start
    deebot-cli clean pause
    deebot-cli dock
    deebot-cli --device \"T50 main\" locate

NOTES:
    The country must match the region your Ecovacs Home app account was created in.
    Repeated failed logins lock the account for about 30 minutes, so retry sparingly.
    On a first login from a new client, Ecovacs emails a one-time code that this tool asks for.";

/// CLI for Ecovacs Deebot robots. Talks to the Ecovacs cloud, not to the robot directly.
#[derive(Parser)]
#[command(name = "deebot-cli", version, after_help = AFTER_HELP)]
struct Cli {
    /// Ecovacs account email
    #[arg(long, env = "ECOVACS_EMAIL", global = true)]
    email: Option<String>,

    /// Two-letter country code of the account's region, e.g. US, IN, GB, DE (default: saved region, else US)
    #[arg(long, env = "ECOVACS_COUNTRY", global = true)]
    country: Option<String>,

    /// Robot to act on, by id, name or nickname (default: the first one)
    #[arg(long, global = true)]
    device: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Test authentication: log in and report success (prints no secrets)
    Login,
    /// List the devices on the account
    Devices,
    /// Make the robot play a sound so you can find it
    Locate,
    /// Send the robot back to its charging dock
    Dock,
    /// Control cleaning: start, pause, resume or stop
    Clean {
        #[arg(value_enum)]
        action: CleanAction,
    },
    /// Show how worn the brushes, filters and mop parts are
    Lifespan,
    /// Show cleaning totals and the last clean's numbers
    Stats,
    /// Make the dock empty the robot's dustbin (the robot must be docked)
    EmptyDustbin,
    /// Show or set the dock's automatic dustbin emptying
    AutoEmpty {
        /// New state; omit to show the current one
        #[arg(value_enum)]
        state: Option<OnOff>,
        /// How often to empty: auto or smart
        #[arg(long, value_enum)]
        frequency: Option<EmptyFrequency>,
    },
    /// List the rooms on the robot's map
    Rooms,
    /// Clean one or more rooms, by name or id (see `rooms`)
    CleanRoom {
        /// Room names (e.g. Kitchen "Living room") or numeric ids
        #[arg(required = true)]
        rooms: Vec<String>,
        /// What to clean with; the robot resets to vacuum-and-mop after a stop, so set it here
        #[arg(long, value_enum)]
        mode: Option<WorkMode>,
    },
    /// Print the robot's work state and position every 2 seconds, to see when a clean starts, moves and ends
    Watch {
        /// How long to watch, in seconds
        #[arg(default_value_t = 90)]
        seconds: u64,
    },
    /// Send any command to the robot and print its full reply (for debugging; nothing checks it is safe)
    Send {
        /// Command name, e.g. clean_V2
        name: String,
        /// The command's data as JSON, e.g. '{"act":"start","content":{"type":"auto"}}'
        data: Option<String>,
    },
    /// Explore the map data the robot sends (read-only, for debugging): prints replies with long values cut
    MapProbe {
        /// Which map piece to request
        #[arg(long, default_value_t = 0)]
        piece: u32,
        /// Also decode the map outline and summarize the shapes in it
        #[arg(long)]
        decode: bool,
    },
    /// Show or set what the robot cleans with: vacuum, mop, both, or mop after vacuum
    WorkMode {
        /// New mode; omit to show the current one
        #[arg(value_enum)]
        mode: Option<WorkMode>,
    },
    /// Show or set how many passes the robot makes per clean
    CleanCount {
        /// New number of passes (1-3); omit to show the current one
        #[arg(value_parser = clap::value_parser!(u8).range(1..=3))]
        count: Option<u8>,
    },
    /// Show the robot's Wi-Fi details (network name, signal, IP)
    Network,
    /// Show the robot's firmware and hardware versions
    Firmware,
    /// Show all supported settings
    Settings,
    /// Show or set the child lock (locks the robot's own buttons)
    ChildLock {
        /// New state; omit to show the current one
        #[arg(value_enum)]
        state: Option<OnOff>,
    },
    /// Show or set automatic extra suction on carpet
    CarpetBoost {
        /// New state; omit to show the current one
        #[arg(value_enum)]
        state: Option<OnOff>,
    },
    /// Show the efficiency mode (read-only for now)
    Efficiency,
    /// Show or set another on/off switch: advanced-mode, sweep-mode, true-detect or voice-assistant
    Switch {
        #[arg(value_enum)]
        name: SwitchName,
        /// New state; omit to show the current one
        #[arg(value_enum)]
        state: Option<OnOff>,
        /// Also print the robot's raw answer (for debugging)
        #[arg(long)]
        raw: bool,
    },
    /// Show or set the robot's voice volume
    Volume {
        /// New volume; omit to show the current one
        level: Option<i64>,
    },
    /// Show or set the suction level
    Fan {
        /// New level; omit to show the current one
        #[arg(value_enum)]
        level: Option<FanLevel>,
    },
    /// Show or set the mop water level
    Water {
        /// New level from 0 to 50; omit to show the current one
        #[arg(value_parser = clap::value_parser!(u8).range(0..=50))]
        level: Option<u8>,
    },
    /// Show the robot's battery, charging state, suction, mop water level and errors
    Status {
        /// Also print each raw response from the robot (useful for debugging)
        #[arg(long)]
        raw: bool,
    },
    /// Delete the saved session so the next command asks for the password again
    Logout,
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum OnOff {
    On,
    Off,
}

impl OnOff {
    fn is_on(self) -> bool {
        self == Self::On
    }
}

fn prompt_line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn mask(id: &str) -> String {
    if id.len() <= 6 {
        return "*".repeat(id.len());
    }
    format!("{}…{}", &id[..3], &id[id.len() - 3..])
}

/// Log in using a saved session when possible, otherwise with the password.
async fn log_in(cli: &Cli) -> Result<Arc<Login>> {
    let identity = session::resolve_identity(cli.email.as_deref(), cli.country.as_deref());
    let email = match identity.email {
        Some(email) => email,
        None => prompt_line("Ecovacs email: ")?,
    };

    if let Some(saved) = &identity.saved {
        match session::resume(&email, &identity.country, saved).await {
            Ok(login) => return Ok(Arc::new(login)),
            Err(err) => eprintln!("Saved session not usable ({err}); logging in with your password"),
        }
    }

    let password = match std::env::var("ECOVACS_PASSWORD") {
        Ok(password) if !password.is_empty() => password,
        _ => rpassword::prompt_password("Ecovacs password: ")?,
    };
    if email.is_empty() || password.is_empty() {
        bail!("an email and password are required");
    }

    let login = session::password_login(&email, &identity.country, &password, || {
        prompt_line("Ecovacs emailed you a verification code, enter it: ")
    })
    .await?;
    Ok(Arc::new(login))
}

async fn show_toggle(robot: &Robot, toggle: &deebot_core::robot::Toggle, state: Option<OnOff>) -> Result<()> {
    match state {
        Some(state) => {
            let now = robot.set_toggle(toggle, state.is_on()).await?;
            println!("{}: {now} (read back from the robot)", toggle.label);
        }
        None => println!("{}: {}", toggle.label, robot.toggle(toggle).await?),
    }
    Ok(())
}

/// Cut long strings and lists so a map reply can be read at a glance.
fn truncated(value: &serde_json::Value) -> serde_json::Value {
    match value {
        Value::String(s) if s.chars().count() > 100 => {
            let head: String = s.chars().take(100).collect();
            Value::String(format!("{head}…({} chars)", s.chars().count()))
        }
        Value::Array(list) if list.len() > 12 => {
            let mut shown: Vec<Value> = list.iter().take(12).map(truncated).collect();
            shown.push(Value::String(format!("…({} items)", list.len())));
            Value::Array(shown)
        }
        Value::Array(list) => Value::Array(list.iter().map(truncated).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), truncated(v))).collect()),
        other => other.clone(),
    }
}

async fn probe_map(robot: &Robot, piece: u32, decode: bool) -> Result<()> {
    let mid = robot.active_map_id().await?;
    println!("map id: {mid}");
    let probes = [
        ("getPos", Some(json!(["chargePos", "deebotPos"]))),
        ("getMajorMap", None),
        ("getMinorMap", Some(json!({ "mid": mid, "type": "ol", "pieceIndex": piece }))),
        ("getMapInfo_V2", Some(json!({ "mid": mid, "type": "0" }))),
        ("getMapTrace", Some(json!({ "pointCount": 200, "traceStart": 0 }))),
    ];
    for (name, args) in probes {
        match robot.raw(name, args).await {
            Ok(reply) => println!("{name}: {}", truncated(reply.get("body").unwrap_or(&reply))),
            Err(err) => println!("{name}: error: {err:#}"),
        }
    }
    if decode {
        summarize_map(robot).await?;
    }
    Ok(())
}

/// Describe the decoded map without printing every coordinate.
async fn summarize_map(robot: &Robot) -> Result<()> {
    let map = robot.map_info().await?;
    let shape = |shape: &deebot_core::map::Shape| format!("id {} ({} points)", shape.id, shape.points.len());
    println!("outlines: {}", map.outlines.iter().map(shape).collect::<Vec<_>>().join(", "));
    println!("rooms: {}", map.rooms.iter().map(shape).collect::<Vec<_>>().join(", "));
    println!("wall lines: {}", map.block_lines.len());
    println!("other entry types: {:?}", map.other_entries);
    match map.bounds() {
        Some((x0, y0, x1, y1)) => println!(
            "bounds (map units): x {x0:.0} to {x1:.0}, y {y0:.0} to {y1:.0}  =  {:.0} x {:.0} pixels",
            (x1 - x0) / deebot_core::map::UNITS_PER_PIXEL,
            (y1 - y0) / deebot_core::map::UNITS_PER_PIXEL
        ),
        None => println!("the map has no shapes"),
    }

    let trace = robot.trace().await?;
    println!("path: {} points", trace.len());
    if let Some(first) = trace.first() {
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for point in &trace {
            (x0, y0, x1, y1) = (x0.min(point.x), y0.min(point.y), x1.max(point.x), y1.max(point.y));
        }
        println!("path bounds (raw): x {x0:.0} to {x1:.0}, y {y0:.0} to {y1:.0}");
        let last = trace[trace.len() - 1];
        println!("path first (raw): {:.0}, {:.0}   last (raw): {:.0}, {:.0}", first.x, first.y, last.x, last.y);
        println!("jumps in the path: {}", trace.iter().filter(|p| !p.connected).count());

        // Which end of the path is the newest? It is the one nearest the robot right now.
        let positions = robot.positions().await?;
        if let Some(pose) = positions.robot {
            // Path points use units ten times finer than positions, so scale them up first
            let pixels_from_robot = |p: &deebot_core::map::TracePoint| {
                ((p.x * 10.0 - pose.x).hypot(p.y * 10.0 - pose.y)) / deebot_core::map::UNITS_PER_PIXEL
            };
            let (from_first, from_last) = (pixels_from_robot(first), pixels_from_robot(&last));
            println!("robot is {from_first:.1} pixels from the path's first point and {from_last:.1} from its last");
            println!(
                "so the newest point is the path's {}",
                if from_first < from_last { "FIRST (newest-first order)" } else { "LAST (oldest-first order)" }
            );
            // Compare the robot's own angle with the way the path is actually going
            println!("robot angle from getPos: {:.0}", pose.angle);
            for pair in trace.windows(2).rev().take(5) {
                let (dx, dy) = (pair[1].x - pair[0].x, pair[1].y - pair[0].y);
                println!(
                    "  step ({:.0},{:.0}) -> ({:.0},{:.0}): travel angle {:.0} deg (atan2 of raw dy,dx), connected={}",
                    pair[0].x, pair[0].y, pair[1].x, pair[1].y, dy.atan2(dx).to_degrees(), pair[1].connected
                );
            }
        }
    }
    Ok(())
}

async fn show_efficiency(robot: &Robot) -> Result<()> {
    // What each number means isn't confirmed, so the raw value is shown
    let efficiency = robot.efficiency().await?;
    match efficiency.value {
        Some(mode) => println!("Efficiency mode: {mode} (raw value; {})", efficiency.raw),
        None => println!("Efficiency mode: unknown ({})", efficiency.raw),
    }
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    if let Command::Logout = cli.command {
        if session::delete_session()? {
            println!("Logged out: saved session deleted");
        } else {
            println!("No saved session");
        }
        return Ok(());
    }

    let login = log_in(&cli).await?;
    let selector = cli.device.as_deref();

    match cli.command {
        Command::Logout => unreachable!("handled above"),
        Command::Login => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let hours = login.credentials.expires_at.saturating_sub(now) / 3600;
            println!(
                "Login OK (region {}, user {}, session valid for about {hours} h)",
                login.country,
                mask(&login.credentials.user_id)
            );
        }
        Command::Devices => {
            let devices = login.devices().await?;
            if devices.is_empty() {
                println!("No devices on this account");
            }
            for device in devices {
                println!(
                    "{}  {}  class={}  company={}",
                    device.did,
                    device.nick.as_deref().filter(|n| !n.is_empty()).unwrap_or(&device.name),
                    device.class,
                    device.company
                );
            }
        }
        Command::Status { raw } => {
            let robot = login.robot(selector).await?;
            let status = robot.status().await?;
            println!("Robot:      {} (class {})", status.name, status.class);
            for line in status.lines {
                println!("{:<11} {}", line.label, line.text);
                if raw {
                    println!("            {}: {}", line.command, line.raw);
                }
            }
        }
        Command::Locate => {
            let robot = login.robot(selector).await?;
            robot.locate().await?;
            println!("Sent: {} should be playing a sound", robot.name());
        }
        Command::Dock => {
            let robot = login.robot(selector).await?;
            robot.dock().await?;
            println!("Sent: {} is returning to the dock", robot.name());
        }
        Command::Clean { action } => {
            let robot = login.robot(selector).await?;
            let done = robot.clean(action).await?;
            println!("Sent: {} {done}", robot.name());
        }
        Command::Stats => {
            let stats = login.robot(selector).await?.stats().await?;
            println!("Total:      {} cleans, area {}, time {}", stats.total_cleans, stats.total_area, stats.total_time);
            println!("Last clean: area {}, time {}", stats.last_area, stats.last_time);
        }
        Command::Lifespan => {
            for part in login.robot(selector).await?.lifespan().await? {
                match (part.percent(), part.left, part.total) {
                    (Some(percent), Some(left), Some(total)) => {
                        println!("{:<18} {percent:>3}%  ({left} of {total} left)", part.name);
                    }
                    _ => println!("{:<18} {}", part.name, part.raw),
                }
            }
        }
        Command::MapProbe { piece, decode } => probe_map(&login.robot(selector).await?, piece, decode).await?,
        Command::Watch { seconds } => {
            let robot = login.robot(selector).await?;
            let started = std::time::Instant::now();
            while started.elapsed().as_secs() < seconds {
                let state = robot.raw("getWorkState", None).await?;
                let work = state.pointer("/body/data/robotState/state").and_then(Value::as_str).unwrap_or("?").to_string();
                let kind = state.pointer("/body/data/robotState/cleanState/type").and_then(Value::as_str).unwrap_or("-").to_string();
                let at = match robot.positions().await.ok().and_then(|p| p.robot) {
                    Some(pose) => format!("at {:.0},{:.0}", pose.x, pose.y),
                    None => "position unknown".to_string(),
                };
                println!("t={:>3}s  {work:<10} {kind:<10} {at}", started.elapsed().as_secs());
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
        Command::CleanRoom { rooms, mode } => {
            let robot = login.robot(selector).await?;
            let known = robot.rooms().await?;
            let mut chosen: Vec<&deebot_core::robot::Room> = Vec::new();
            for wanted in &rooms {
                let key = wanted.trim().to_lowercase();
                let Some(found) = known.iter().find(|r| r.id.to_string() == key || r.name.to_lowercase() == key) else {
                    bail!("no room \"{wanted}\"; rooms are: {}", known.iter().map(|r| format!("{} ({})", r.label(), r.id)).collect::<Vec<_>>().join(", "));
                };
                if !chosen.iter().any(|r| r.id == found.id) {
                    chosen.push(found);
                }
            }
            let ids: Vec<i64> = chosen.iter().map(|r| r.id).collect();
            let value = robot.clean_rooms(&ids, mode).await?;
            println!("Sent: {} started cleaning {}", robot.name(), chosen.iter().map(|r| r.label()).collect::<Vec<_>>().join(", "));
            println!("Sent value: {value}");
        }
        Command::Send { name, data } => {
            let args = data.map(|text| serde_json::from_str::<Value>(&text)).transpose().map_err(|e| anyhow::anyhow!("data is not valid JSON: {e}"))?;
            let reply = login.robot(selector).await?.raw(&name, args).await?;
            println!("{}", serde_json::to_string_pretty(reply.get("body").unwrap_or(&reply))?);
        }
        Command::Rooms => {
            let robot = login.robot(selector).await?;
            let rooms = robot.rooms().await?;
            if rooms.is_empty() {
                println!("No rooms on the active map");
            }
            // The map gives each room's outline; its bounding box is shown as x1,y1,x2,y2 in map units
            let map = robot.map_info().await.ok();
            for room in &rooms {
                let shape = map.as_ref().and_then(|m| m.rooms.iter().find(|s| s.id == room.id.to_string()));
                let area = shape.map(|s| {
                    let (xs, ys): (Vec<f32>, Vec<f32>) = s.points.iter().map(|p| (p.x, p.y)).unzip();
                    let fold = |v: &[f32], f: fn(f32, f32) -> f32, init: f32| v.iter().copied().fold(init, f);
                    format!(
                        "{:.0},{:.0},{:.0},{:.0}",
                        fold(&xs, f32::min, f32::MAX), fold(&ys, f32::min, f32::MAX), fold(&xs, f32::max, f32::MIN), fold(&ys, f32::max, f32::MIN)
                    )
                });
                println!("{:>3}  {:<14} {}", room.id, room.label(), area.unwrap_or_default());
            }
        }
        Command::WorkMode { mode } => {
            let robot = login.robot(selector).await?;
            if let Some(mode) = mode {
                robot.set_work_mode(mode).await?;
            }
            let reading = robot.work_mode().await?;
            let note = if mode.is_some() { " (read back from the robot)" } else { "" };
            match reading.value {
                Some(value) => println!("Work mode: {} ({value}){note}", work_mode_name(value)),
                None => println!("Work mode: unknown ({})", reading.raw),
            }
        }
        Command::CleanCount { count } => {
            let robot = login.robot(selector).await?;
            if let Some(count) = count {
                robot.set_clean_count(count).await?;
            }
            let reading = robot.clean_count().await?;
            let note = if count.is_some() { " (read back from the robot)" } else { "" };
            match reading.value {
                Some(value) => println!("Clean count: {value}{note}"),
                None => println!("Clean count: unknown ({})", reading.raw),
            }
        }
        Command::Network => {
            let network = login.robot(selector).await?.network().await?;
            println!("Network: {}", network.ssid);
            println!("IP:      {}", network.ip);
            println!("Signal:  {} (raw value from the robot)", network.rssi);
            println!("MAC:     {}", network.mac);
        }
        Command::Firmware => {
            let firmware = login.robot(selector).await?.firmware().await?;
            println!("Firmware: {}", firmware.firmware);
            println!("Hardware: {}", firmware.hardware);
            println!("Software: {}", firmware.software);
        }
        Command::EmptyDustbin => {
            login.robot(selector).await?.empty_dustbin().await?;
            println!("Sent: the dock is emptying the dustbin");
        }
        Command::AutoEmpty { state, frequency } => {
            let robot = login.robot(selector).await?;
            let changing = state.is_some() || frequency.is_some();
            if changing {
                robot.set_auto_empty(state.map(OnOff::is_on), frequency).await?;
            }
            let auto_empty = robot.auto_empty().await?;
            match auto_empty.enabled {
                None => println!("Auto-empty: unknown ({})", auto_empty.raw),
                Some(enabled) => println!(
                    "Auto-empty: {}{}{}",
                    if enabled { "on" } else { "off" },
                    auto_empty.frequency.map_or(String::new(), |f| format!(", frequency {f}")),
                    if changing { " (read back from the robot)" } else { "" }
                ),
            }
        }
        Command::Settings => {
            let robot = login.robot(selector).await?;
            // A switch the robot lacks shouldn't hide the others
            for toggle in ALL_TOGGLES {
                if let Err(err) = show_toggle(&robot, toggle, None).await {
                    println!("{}: unavailable ({err:#})", toggle.label);
                }
            }
            show_efficiency(&robot).await?;
        }
        Command::ChildLock { state } => show_toggle(&login.robot(selector).await?, &CHILD_LOCK, state).await?,
        Command::CarpetBoost { state } => show_toggle(&login.robot(selector).await?, &CARPET_BOOST, state).await?,
        Command::Efficiency => show_efficiency(&login.robot(selector).await?).await?,
        Command::Switch { name, state, raw } => {
            let robot = login.robot(selector).await?;
            if raw {
                println!("{}: {}", name.toggle().get_command(), robot.toggle_raw(name.toggle()).await?);
            }
            show_toggle(&robot, name.toggle(), state).await?;
        }
        Command::Volume { level } => {
            let robot = login.robot(selector).await?;
            match level {
                None => {
                    let volume = robot.volume().await?;
                    let range = volume.total.map_or(String::new(), |t| format!(" (range 0-{t})"));
                    match volume.current {
                        Some(current) => println!("Volume: {current}{range}"),
                        None => println!("Volume: {}", volume.raw),
                    }
                }
                Some(level) => {
                    robot.set_volume(level).await?;
                    println!("Sent: volume set to {level}");
                }
            }
        }
        Command::Fan { level } => {
            let robot = login.robot(selector).await?;
            match level {
                Some(level) => {
                    robot.set_fan(level).await?;
                    println!("Sent: suction set to {}", fan_speed_name(level.value()));
                }
                None => {
                    let reading = robot.fan().await?;
                    match reading.value {
                        Some(speed) => println!("Suction: {} ({speed})", fan_speed_name(speed)),
                        None => println!("Suction: {}", reading.raw),
                    }
                }
            }
        }
        Command::Water { level } => {
            let robot = login.robot(selector).await?;
            match level {
                Some(level) => {
                    robot.set_water(level).await?;
                    println!("Sent: mop water set to {level}");
                }
                None => {
                    let reading = robot.water().await?;
                    match reading.value {
                        Some(amount) => println!("Mop water: {amount} (range 0-50)"),
                        None => println!("Mop water: {}", reading.raw),
                    }
                }
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(err) = run(Cli::parse()).await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
