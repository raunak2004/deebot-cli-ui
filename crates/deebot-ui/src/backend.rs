//! A background thread that talks to the robot, so the window never freezes on a slow request.
//!
//! The UI sends [`Cmd`]s; the worker runs them and replies with [`Event`]s, then asks egui to repaint.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use deebot_core::map::TracePoint;
use deebot_core::robot::{
    AutoEmpty, MapView, Positions, CleanAction, EmptyFrequency, FanLevel, Firmware, Network, Part, Room, Stats, Status, ToggleState,
    Volume, WorkMode, ALL_TOGGLES,
};
use deebot_core::{session, Robot};
use eframe::egui;

/// Everything the UI shows about the robot; filled in piece by piece as replies arrive.
#[derive(Default)]
pub struct Data {
    pub status: Option<Status>,
    /// One entry per [`ALL_TOGGLES`], in the same order
    pub toggles: Vec<Result<ToggleState, String>>,
    pub fan: Option<i64>,
    pub water: Option<i64>,
    pub work_mode: Option<i64>,
    pub clean_count: Option<i64>,
    pub volume: Option<Volume>,
    pub auto_empty: Option<AutoEmpty>,
    pub parts: Vec<Part>,
    pub stats: Option<Stats>,
    pub network: Option<Network>,
    pub firmware: Option<Firmware>,
    pub rooms: Vec<Room>,
    pub map: Option<MapView>,
    pub positions: Positions,
    pub trace: Vec<TracePoint>,
}

pub enum Cmd {
    /// Log in with the saved session, if there is one
    TryResume,
    Login { email: String, password: String, country: String },
    /// The emailed verification code Ecovacs asked for
    Code(String),
    Logout,
    RefreshStatus,
    RefreshAll,
    /// Position and path for the live map; `trace_len` is how many path points the UI already has
    RefreshLive { trace_len: usize },
    Clean(CleanAction),
    /// Clean these rooms (id and name), set to the given work mode first
    CleanRooms(Vec<(i64, String)>, WorkMode),
    Dock,
    Locate,
    EmptyDustbin,
    SetToggle(usize, bool),
    SetFan(FanLevel),
    SetWater(u8),
    SetWorkMode(WorkMode),
    SetCleanCount(u8),
    SetVolume(i64),
    SetAutoEmpty(Option<bool>, Option<EmptyFrequency>),
}

pub enum Event {
    /// No usable saved session: show the login form
    NeedLogin,
    /// Ecovacs wants a one-time code from the user's email
    NeedCode,
    LoggedIn { robot: String },
    LoginFailed(String),
    /// Something finished: a message for the status bar
    Notice(Result<String, String>),
    Update(Box<dyn FnOnce(&mut Data) + Send>),
    /// A robot command finished (successfully or not)
    Done,
}

/// The UI's handle to the worker thread.
pub struct Backend {
    cmd_tx: Sender<Cmd>,
    pub events: Receiver<Event>,
}

impl Backend {
    pub fn start(ctx: egui::Context) -> Self {
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, events) = channel();
        std::thread::spawn(move || worker(cmd_rx, event_tx, ctx));
        Self { cmd_tx, events }
    }

    pub fn send(&self, cmd: Cmd) {
        // The worker only stops when the app closes, so a failed send is safe to ignore
        let _ = self.cmd_tx.send(cmd);
    }
}

/// Sends events to the UI and wakes it up to draw them.
#[derive(Clone)]
struct Emitter {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Emitter {
    fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }

    fn update(&self, apply: impl FnOnce(&mut Data) + Send + 'static) {
        self.emit(Event::Update(Box::new(apply)));
    }

    fn notice(&self, result: Result<String, String>) {
        self.emit(Event::Notice(result));
    }
}

type RobotSlot = Arc<Mutex<Option<Arc<Robot>>>>;

fn worker(cmd_rx: Receiver<Cmd>, event_tx: Sender<Event>, ctx: egui::Context) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .expect("could not start the async runtime");
    let emitter = Emitter { tx: event_tx, ctx };
    let robot: RobotSlot = Arc::new(Mutex::new(None));
    // Carries the verification code from the UI to a login that is waiting for it
    let (code_tx, code_rx) = channel::<String>();
    let code_rx = Arc::new(Mutex::new(code_rx));

    for cmd in cmd_rx {
        let emitter = emitter.clone();
        match cmd {
            Cmd::Code(code) => {
                let _ = code_tx.send(code);
            }
            Cmd::Logout => {
                *robot.lock().expect("robot lock") = None;
                match session::delete_session() {
                    Ok(_) => emitter.emit(Event::NeedLogin),
                    Err(err) => emitter.notice(Err(format!("{err:#}"))),
                }
            }
            Cmd::TryResume => {
                let slot = Arc::clone(&robot);
                runtime.spawn(async move {
                    let identity = session::resolve_identity(None, None);
                    let resumed = match (&identity.email, &identity.saved) {
                        (Some(email), Some(saved)) => session::resume(email, &identity.country, saved).await.ok(),
                        _ => None,
                    };
                    match resumed {
                        Some(login) => connect(login, &slot, &emitter).await,
                        None => {
                            emitter.emit(Event::NeedLogin);
                            emitter.emit(Event::Done);
                        }
                    }
                });
            }
            Cmd::Login { email, password, country } => {
                let slot = Arc::clone(&robot);
                let code_rx = Arc::clone(&code_rx);
                runtime.spawn(async move {
                    let ask = emitter.clone();
                    let login = session::password_login(&email, &country.to_uppercase(), &password, move || {
                        ask.emit(Event::NeedCode);
                        // Wait on the UI's code without stalling the async runtime
                        tokio::task::block_in_place(|| {
                            code_rx.lock().expect("code lock").recv().map_err(|_| anyhow!("login was cancelled"))
                        })
                    })
                    .await;
                    match login {
                        Ok(login) => connect(login, &slot, &emitter).await,
                        Err(err) => {
                            emitter.emit(Event::LoginFailed(format!("{err:#}")));
                            emitter.emit(Event::Done);
                        }
                    }
                });
            }
            robot_cmd => {
                let Some(current) = robot.lock().expect("robot lock").clone() else {
                    emitter.notice(Err("not logged in".to_string()));
                    emitter.emit(Event::Done);
                    continue;
                };
                // Live-map updates run quietly: they neither show the busy bar nor count as pending
                let quiet = matches!(robot_cmd, Cmd::RefreshLive { .. });
                runtime.spawn(async move {
                    run_robot_cmd(robot_cmd, &current, &emitter).await;
                    if !quiet {
                        emitter.emit(Event::Done);
                    }
                });
            }
        }
    }
}

/// Pick the robot on a fresh login, announce it, and load everything about it.
async fn connect(login: deebot_core::Login, slot: &RobotSlot, emitter: &Emitter) {
    let login = Arc::new(login);
    match login.robot(None).await {
        Ok(robot) => {
            let robot = Arc::new(robot);
            let name = robot.name().to_string();
            *slot.lock().expect("robot lock") = Some(Arc::clone(&robot));
            emitter.emit(Event::LoggedIn { robot: name });
            refresh_status(&robot, emitter).await;
            refresh_all(&robot, emitter).await;
        }
        Err(err) => emitter.emit(Event::LoginFailed(format!("{err:#}"))),
    }
    emitter.emit(Event::Done);
}

async fn run_robot_cmd(cmd: Cmd, robot: &Robot, emitter: &Emitter) {
    match cmd {
        Cmd::RefreshStatus => refresh_status(robot, emitter).await,
        Cmd::RefreshAll => {
            refresh_status(robot, emitter).await;
            refresh_all(robot, emitter).await;
        }
        Cmd::RefreshLive { trace_len } => {
            if let Ok(positions) = robot.positions().await {
                emitter.update(move |data| data.positions = positions);
            }
            if let Ok((from, points)) = robot.trace_since(trace_len).await {
                emitter.update(move |data| {
                    data.trace.truncate(from);
                    data.trace.extend(points);
                });
            }
        }
        Cmd::Clean(action) => {
            report(emitter, robot.clean(action).await.map(|done| format!("{} {done}", robot.name())));
            refresh_status(robot, emitter).await;
        }
        Cmd::CleanRooms(rooms, mode) => {
            let ids: Vec<i64> = rooms.iter().map(|(id, _)| *id).collect();
            let names = rooms.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>().join(", ");
            report(emitter, robot.clean_rooms(&ids, Some(mode)).await.map(|_| format!("{} started cleaning {names}", robot.name())));
            fetch(emitter, robot.work_mode().await, |data, value| data.work_mode = value);
            refresh_status(robot, emitter).await;
        }
        Cmd::Dock => {
            report(emitter, robot.dock().await.map(|()| format!("{} is returning to the dock", robot.name())));
            refresh_status(robot, emitter).await;
        }
        Cmd::Locate => {
            report(emitter, robot.locate().await.map(|()| format!("{} should be playing a sound", robot.name())));
        }
        Cmd::EmptyDustbin => {
            report(emitter, robot.empty_dustbin().await.map(|()| "The dock is emptying the dustbin".to_string()));
        }
        Cmd::SetToggle(index, on) => {
            let toggle = ALL_TOGGLES[index];
            let result = robot.set_toggle(toggle, on).await;
            report(emitter, result.as_ref().map(|state| format!("{}: {state}", toggle.label)).map_err(|e| anyhow!("{e:#}")));
            emitter.update(move |data| {
                if let (Some(slot), Ok(state)) = (data.toggles.get_mut(index), result) {
                    *slot = Ok(state);
                }
            });
        }
        Cmd::SetFan(level) => {
            report(emitter, robot.set_fan(level).await.map(|()| "Suction updated".to_string()));
            fetch(emitter, robot.fan().await, |data, value| data.fan = value);
        }
        Cmd::SetWater(level) => {
            report(emitter, robot.set_water(level).await.map(|()| "Mop water updated".to_string()));
            fetch(emitter, robot.water().await, |data, value| data.water = value);
        }
        Cmd::SetWorkMode(mode) => {
            report(emitter, robot.set_work_mode(mode).await.map(|()| "Work mode updated".to_string()));
            fetch(emitter, robot.work_mode().await, |data, value| data.work_mode = value);
        }
        Cmd::SetCleanCount(count) => {
            report(emitter, robot.set_clean_count(count).await.map(|()| "Clean count updated".to_string()));
            fetch(emitter, robot.clean_count().await, |data, value| data.clean_count = value);
        }
        Cmd::SetVolume(level) => {
            report(emitter, robot.set_volume(level).await.map(|()| "Volume updated".to_string()));
            if let Ok(volume) = robot.volume().await {
                emitter.update(move |data| data.volume = Some(volume));
            }
        }
        Cmd::SetAutoEmpty(state, frequency) => {
            report(emitter, robot.set_auto_empty(state, frequency).await.map(|()| "Auto-empty updated".to_string()));
            if let Ok(auto_empty) = robot.auto_empty().await {
                emitter.update(move |data| data.auto_empty = Some(auto_empty));
            }
        }
        // Handled by the worker loop before reaching here
        Cmd::TryResume | Cmd::Login { .. } | Cmd::Code(_) | Cmd::Logout => {}
    }
}

fn report(emitter: &Emitter, result: Result<String>) {
    emitter.notice(result.map_err(|err| format!("{err:#}")));
}

/// Store a numeric reading in [`Data`] if it was read successfully.
fn fetch(emitter: &Emitter, result: Result<deebot_core::robot::Reading>, store: impl FnOnce(&mut Data, Option<i64>) + Send + 'static) {
    if let Ok(reading) = result {
        emitter.update(move |data| store(data, reading.value));
    }
}

async fn refresh_status(robot: &Robot, emitter: &Emitter) {
    match robot.status().await {
        Ok(status) => emitter.update(move |data| data.status = Some(status)),
        Err(err) => emitter.notice(Err(format!("{err:#}"))),
    }
}

/// Load every setting and reading. Individual failures leave that piece empty rather than stopping.
async fn refresh_all(robot: &Robot, emitter: &Emitter) {
    let mut toggles = Vec::new();
    for toggle in ALL_TOGGLES {
        toggles.push(robot.toggle(toggle).await.map_err(|err| format!("{err:#}")));
    }
    emitter.update(move |data| data.toggles = toggles);

    fetch(emitter, robot.fan().await, |data, value| data.fan = value);
    fetch(emitter, robot.water().await, |data, value| data.water = value);
    fetch(emitter, robot.work_mode().await, |data, value| data.work_mode = value);
    fetch(emitter, robot.clean_count().await, |data, value| data.clean_count = value);
    if let Ok(volume) = robot.volume().await {
        emitter.update(move |data| data.volume = Some(volume));
    }
    if let Ok(auto_empty) = robot.auto_empty().await {
        emitter.update(move |data| data.auto_empty = Some(auto_empty));
    }
    if let Ok(parts) = robot.lifespan().await {
        emitter.update(move |data| data.parts = parts);
    }
    if let Ok(stats) = robot.stats().await {
        emitter.update(move |data| data.stats = Some(stats));
    }
    if let Ok(network) = robot.network().await {
        emitter.update(move |data| data.network = Some(network));
    }
    if let Ok(firmware) = robot.firmware().await {
        emitter.update(move |data| data.firmware = Some(firmware));
    }
    if let Ok(rooms) = robot.rooms().await {
        emitter.update(move |data| data.rooms = rooms);
    }
    if let Ok(map) = robot.map_view().await {
        emitter.update(move |data| data.map = Some(map));
    }
    if let Ok(positions) = robot.positions().await {
        emitter.update(move |data| data.positions = positions);
    }
    if let Ok((_, points)) = robot.trace_since(0).await {
        emitter.update(move |data| data.trace = points);
    }
}
