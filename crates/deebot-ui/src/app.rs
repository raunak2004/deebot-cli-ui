//! The window: login, then tabs for controlling, configuring and checking on the robot.

use std::time::{Duration, Instant};

use deebot_core::robot::{
    fan_speed_name, work_mode_name, CleanAction, EmptyFrequency, FanLevel, ToggleState, WorkMode, ALL_TOGGLES,
};
use deebot_core::session;
use eframe::egui::{self, vec2, Align2, Color32, CornerRadius, Frame, Id, Margin, RichText, Shadow, Stroke};

use crate::backend::{Backend, Cmd, Data, Event};
use crate::mapview;
use crate::platform;
use crate::theme::{self, ACCENT, BAD, BG, CARD, OK, TEXT_WEAK, WARN};

/// How often the status refreshes on its own while idle.
const AUTO_REFRESH: Duration = Duration::from_secs(20);
/// How long a result message stays on screen.
const TOAST_TIME: Duration = Duration::from_secs(5);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Control,
    Map,
    Settings,
    Maintenance,
    Rooms,
}

const TABS: [(Tab, &str); 5] = [
    (Tab::Control, "Control"),
    (Tab::Map, "Map"),
    (Tab::Settings, "Settings"),
    (Tab::Maintenance, "Maintenance"),
    (Tab::Rooms, "Rooms"),
];

/// How often the live map updates while the robot cleans, and while it just sits there.
const LIVE_WHILE_CLEANING: Duration = Duration::from_secs(3);
const LIVE_WHILE_IDLE: Duration = Duration::from_secs(15);

enum Screen {
    /// Trying the saved session
    Starting,
    Login,
    Main,
}

pub struct DeebotApp {
    backend: Backend,
    data: Data,
    screen: Screen,
    tab: Tab,

    email: String,
    password: String,
    country: String,
    code: String,
    need_code: bool,
    login_error: Option<String>,

    robot_name: String,
    /// Commands sent whose reply hasn't arrived yet
    pending: usize,
    /// The last result, shown as a toast: (succeeded, text)
    notice: Option<(bool, String)>,
    notice_at: Instant,
    confirm_start: bool,
    /// Rooms ticked on the Rooms tab, and whether the Start button is waiting for a confirming tap
    selected_rooms: Vec<i64>,
    confirm_rooms: bool,
    /// What a room clean is done with; the robot resets its own setting after each stop
    room_mode: WorkMode,
    /// Slider values being dragged, sent only when released
    water_edit: Option<i64>,
    volume_edit: Option<i64>,
    last_status: Instant,
    last_live: Instant,
}

impl DeebotApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let backend = Backend::start(cc.egui_ctx.clone());
        backend.send(Cmd::TryResume);
        let saved = session::load_session();
        Self {
            backend,
            data: Data::default(),
            screen: Screen::Starting,
            tab: Tab::Control,
            email: saved.as_ref().map(|s| s.email.clone()).unwrap_or_default(),
            password: String::new(),
            country: saved.map(|s| s.country).unwrap_or_else(|| "US".to_string()),
            code: String::new(),
            need_code: false,
            login_error: None,
            robot_name: String::new(),
            pending: 1,
            notice: None,
            notice_at: Instant::now(),
            confirm_start: false,
            selected_rooms: Vec::new(),
            confirm_rooms: false,
            room_mode: WorkMode::Vacuum,
            water_edit: None,
            volume_edit: None,
            last_status: Instant::now(),
            // Far enough back that the first visit to the Map tab updates at once
            last_live: Instant::now().checked_sub(Duration::from_secs(3600)).unwrap_or_else(Instant::now),
        }
    }

    fn cmd(&mut self, cmd: Cmd) {
        if !matches!(cmd, Cmd::Code(_) | Cmd::Logout | Cmd::RefreshLive { .. }) {
            self.pending += 1;
        }
        if matches!(cmd, Cmd::RefreshStatus | Cmd::RefreshAll) {
            self.last_status = Instant::now();
        }
        self.backend.send(cmd);
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.backend.events.try_recv() {
            match event {
                Event::NeedLogin => self.screen = Screen::Login,
                Event::NeedCode => self.need_code = true,
                Event::LoggedIn { robot } => {
                    self.robot_name = robot;
                    self.screen = Screen::Main;
                    self.need_code = false;
                    self.login_error = None;
                    self.last_status = Instant::now();
                }
                Event::LoginFailed(message) => {
                    self.login_error = Some(message);
                    self.need_code = false;
                    self.screen = Screen::Login;
                }
                Event::Notice(result) => {
                    self.notice = Some(match result {
                        Ok(text) => (true, text),
                        Err(text) => (false, text),
                    });
                    self.notice_at = Instant::now();
                }
                Event::Update(apply) => apply(&mut self.data),
                Event::Done => self.pending = self.pending.saturating_sub(1),
            }
        }
    }

    fn login_screen(&mut self, ui: &mut egui::Ui) {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Deebot").size(34.0).strong().color(ACCENT));
            ui.label(RichText::new("Sign in with your Ecovacs account").color(TEXT_WEAK));
            ui.add_space(14.0);
            // Up to 420 wide on a computer, but never wider than a phone screen
            let width = ui.available_width().min(420.0);
            ui.allocate_ui(vec2(width, 0.0), |ui| {
                theme::card(ui, None, |ui| self.login_form(ui));
            });
        });
    }

    fn login_form(&mut self, ui: &mut egui::Ui) {
        // Leave room for the label column so the boxes fit narrow screens too
        let field_width = (ui.available_width() - 110.0).clamp(120.0, 260.0);
        egui::Grid::new("login").num_columns(2).spacing([14.0, 12.0]).show(ui, |ui| {
            ui.label("Email");
            ui.add(egui::TextEdit::singleline(&mut self.email).desired_width(field_width));
            ui.end_row();
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(field_width));
            ui.end_row();
            ui.label("Country");
            ui.add(egui::TextEdit::singleline(&mut self.country).desired_width(60.0).char_limit(2));
            ui.end_row();
        });
        ui.label(RichText::new("Country must match your Ecovacs app's region, e.g. US, IN, GB, DE.").color(TEXT_WEAK));
        ui.label(RichText::new("Your password is used once and never saved.").color(TEXT_WEAK));
        ui.add_space(6.0);

        if self.need_code {
            ui.separator();
            ui.label("Ecovacs emailed you a one-time code. Enter it to verify this device:");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.code).desired_width(160.0));
                if theme::action(ui, "Verify", ACCENT).clicked() && !self.code.trim().is_empty() {
                    let code = std::mem::take(&mut self.code);
                    self.cmd(Cmd::Code(code));
                }
            });
        } else {
            let can_sign_in = self.pending == 0 && !self.email.is_empty() && !self.password.is_empty();
            ui.horizontal(|ui| {
                let button = ui.add_enabled_ui(can_sign_in, |ui| theme::action(ui, "Sign in", ACCENT)).inner;
                if button.clicked() {
                    self.login_error = None;
                    let password = std::mem::take(&mut self.password);
                    self.cmd(Cmd::Login { email: self.email.clone(), password, country: self.country.clone() });
                }
                if self.pending > 0 {
                    ui.spinner();
                    ui.label(RichText::new("Working…").color(TEXT_WEAK));
                }
            });
        }
        if let Some(error) = &self.login_error {
            ui.add_space(6.0);
            ui.colored_label(BAD, error);
            ui.label(RichText::new("Repeated wrong passwords lock the account for about 30 minutes.").color(TEXT_WEAK));
        }
    }

    fn control_tab(&mut self, ui: &mut egui::Ui, out: &mut Vec<Cmd>) {
        let Some(status) = &self.data.status else {
            theme::card(ui, None, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading status…");
                });
            });
            return;
        };
        // Copy what the cards need so the data isn't borrowed while buttons change the app
        let battery = status.battery;
        let docked = status.docked;
        let paused = status.paused;
        let robot_state = status.robot_state.clone();
        let line = |label: &str| status.lines.iter().find(|l| l.label == label).map(|l| l.text.clone());
        let errors = line("Errors").filter(|e| e != "none");
        let suction = line("Suction");
        let water = line("Mop water");

        let (headline, color, pulsing) = if errors.is_some() {
            ("Needs attention".to_string(), BAD, false)
        } else {
            match (robot_state.as_deref(), paused, docked) {
                (Some("cleaning"), false, _) => ("Cleaning".to_string(), ACCENT, true),
                (Some(_), true, _) => ("Paused".to_string(), WARN, false),
                (_, _, Some(true)) => ("Docked".to_string(), OK, false),
                (Some(state), _, _) => {
                    let mut chars = state.chars();
                    (chars.next().map_or(String::new(), |c| c.to_uppercase().chain(chars).collect()), TEXT_WEAK, false)
                }
                _ => ("Unknown".to_string(), TEXT_WEAK, false),
            }
        };

        theme::card(ui, None, |ui| {
            ui.horizontal(|ui| {
                let ring = if ui.available_width() < 420.0 { 124.0 } else { 156.0 };
                theme::battery_ring(ui, battery, ring);
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new(&self.robot_name).size(26.0).strong());
                    ui.horizontal(|ui| {
                        theme::pulse_dot(ui, color, pulsing);
                        ui.label(RichText::new(&headline).size(17.0).color(color));
                    });
                    ui.label(RichText::new(match docked {
                        Some(true) => "On the dock",
                        Some(false) => "Off the dock",
                        None => "Dock state unknown",
                    })
                    .color(TEXT_WEAK));
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if let Some(suction) = &suction {
                            theme::chip(ui, &format!("Suction {suction}"));
                        }
                        if let Some(water) = &water {
                            theme::chip(ui, &format!("Mop {water}"));
                        }
                    });
                    if let Some(errors) = &errors {
                        ui.colored_label(BAD, format!("Errors: {errors}"));
                    }
                });
            });
        });

        let neutral = Color32::from_rgb(70, 78, 96);

        ui.add_space(14.0);
        theme::card(ui, Some("Cleaning"), |ui| {
            if self.confirm_start {
                ui.label("Start a whole-home clean?");
                match theme::button_grid(ui, &[("Yes, start", ACCENT, true), ("Cancel", neutral, true)]) {
                    Some(0) => {
                        out.push(Cmd::Clean(CleanAction::Start));
                        self.confirm_start = false;
                    }
                    Some(_) => self.confirm_start = false,
                    None => {}
                }
            } else {
                let buttons = [("Start", ACCENT, true), ("Pause", neutral, true), ("Resume", neutral, true), ("Stop", BAD, true)];
                match theme::button_grid(ui, &buttons) {
                    Some(0) => self.confirm_start = true,
                    Some(1) => out.push(Cmd::Clean(CleanAction::Pause)),
                    Some(2) => out.push(Cmd::Clean(CleanAction::Resume)),
                    Some(_) => out.push(Cmd::Clean(CleanAction::Stop)),
                    None => {}
                }
            }
        });

        ui.add_space(14.0);
        theme::card(ui, Some("Robot"), |ui| {
            let buttons = [("Return to dock", neutral, true), ("Find robot", neutral, true), ("Empty dustbin", neutral, docked == Some(true))];
            match theme::button_grid(ui, &buttons) {
                Some(0) => out.push(Cmd::Dock),
                Some(1) => out.push(Cmd::Locate),
                Some(_) => out.push(Cmd::EmptyDustbin),
                None => {}
            }
            ui.label(RichText::new("Find robot makes it play a sound. Emptying the dustbin needs the robot on its dock.").color(TEXT_WEAK));
        });
    }

    /// Whether the status says the robot is out cleaning right now.
    fn is_cleaning(&self) -> bool {
        self.data.status.as_ref().is_some_and(|s| s.robot_state.as_deref() == Some("cleaning") && !s.paused)
    }

    fn map_tab(&mut self, ui: &mut egui::Ui) {
        let cleaning = self.is_cleaning();
        theme::card(ui, Some("Map"), |ui| match &self.data.map {
            Some(view) => {
                let live = mapview::Live { positions: &self.data.positions, trace: &self.data.trace, cleaning };
                mapview::draw(ui, view, &self.data.rooms, live);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(if cleaning {
                        "Live: updating every few seconds while the robot cleans."
                    } else {
                        "The robot is not cleaning, so the map only refreshes occasionally."
                    })
                    .color(TEXT_WEAK),
                );
            }
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading the map…").color(TEXT_WEAK));
                });
            }
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui, out: &mut Vec<Cmd>) {
        theme::card(ui, Some("Cleaning"), |ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 210.0).clamp(80.0, 200.0);

            let current = self.data.fan;
            theme::row(ui, "Suction", |ui| {
                egui::ComboBox::from_id_salt("fan").selected_text(current.map_or("…", fan_speed_name)).show_ui(ui, |ui| {
                    for level in FanLevel::ALL {
                        if ui.selectable_label(current == Some(level.value()), fan_speed_name(level.value())).clicked() {
                            out.push(Cmd::SetFan(level));
                        }
                    }
                });
            });

            let current = self.data.work_mode;
            theme::row(ui, "Work mode", |ui| {
                egui::ComboBox::from_id_salt("work-mode")
                    .selected_text(current.map_or("…", work_mode_name))
                    .show_ui(ui, |ui| {
                        for mode in WorkMode::ALL {
                            if ui.selectable_label(current == Some(mode.value()), work_mode_name(mode.value())).clicked() {
                                out.push(Cmd::SetWorkMode(mode));
                            }
                        }
                    });
            });

            let current = self.data.clean_count;
            theme::row(ui, "Passes per clean", |ui| {
                egui::ComboBox::from_id_salt("clean-count")
                    .selected_text(current.map_or("…".to_string(), |c| c.to_string()))
                    .show_ui(ui, |ui| {
                        for count in 1..=3u8 {
                            if ui.selectable_label(current == Some(i64::from(count)), count.to_string()).clicked() {
                                out.push(Cmd::SetCleanCount(count));
                            }
                        }
                    });
            });

            let water = self.data.water;
            theme::row(ui, "Mop water", |ui| match water {
                Some(water) => {
                    let mut value = self.water_edit.unwrap_or(water);
                    let response = ui.add(egui::Slider::new(&mut value, 0..=50));
                    if response.changed() {
                        self.water_edit = Some(value);
                    }
                    if response.drag_stopped() || (response.changed() && !response.dragged()) {
                        self.water_edit = None;
                        out.push(Cmd::SetWater(value as u8));
                    }
                }
                None => {
                    ui.label("…");
                }
            });

            let volume = self.data.volume.clone();
            theme::row(ui, "Voice volume", |ui| match &volume {
                Some(volume) => {
                    let mut value = self.volume_edit.or(volume.current).unwrap_or(0);
                    let response = ui.add(egui::Slider::new(&mut value, 0..=volume.total.unwrap_or(10)));
                    if response.changed() {
                        self.volume_edit = Some(value);
                    }
                    if response.drag_stopped() || (response.changed() && !response.dragged()) {
                        self.volume_edit = None;
                        out.push(Cmd::SetVolume(value));
                    }
                }
                None => {
                    ui.label("…");
                }
            });
        });

        ui.add_space(14.0);
        theme::card(ui, Some("Switches"), |ui| {
            for (index, toggle) in ALL_TOGGLES.iter().enumerate() {
                match self.data.toggles.get(index) {
                    Some(Ok(state @ (ToggleState::On | ToggleState::Off))) => {
                        let mut on = *state == ToggleState::On;
                        theme::row(ui, toggle.label, |ui| {
                            if theme::switch(ui, &mut on).changed() {
                                out.push(Cmd::SetToggle(index, on));
                            }
                        });
                    }
                    Some(Ok(state)) => {
                        ui.label(format!("{}: {state}", toggle.label));
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new(format!("{}: unavailable ({err})", toggle.label)).color(TEXT_WEAK));
                    }
                    None => {
                        ui.label(RichText::new(format!("{}: …", toggle.label)).color(TEXT_WEAK));
                    }
                }
            }
        });

        ui.add_space(14.0);
        theme::card(ui, Some("Dock"), |ui| match &self.data.auto_empty {
            Some(auto_empty) => match auto_empty.enabled {
                Some(enabled) => {
                    let mut on = enabled;
                    theme::row(ui, "Empty the dustbin automatically", |ui| {
                        if theme::switch(ui, &mut on).changed() {
                            out.push(Cmd::SetAutoEmpty(Some(on), None));
                        }
                    });
                    let frequency = auto_empty.frequency.clone();
                    theme::row(ui, "How often", |ui| {
                        egui::ComboBox::from_id_salt("auto-empty-frequency")
                            .selected_text(frequency.clone().unwrap_or_else(|| "…".to_string()))
                            .show_ui(ui, |ui| {
                                for option in [EmptyFrequency::Auto, EmptyFrequency::Smart] {
                                    if ui.selectable_label(frequency.as_deref() == Some(option.value()), option.value()).clicked() {
                                        out.push(Cmd::SetAutoEmpty(None, Some(option)));
                                    }
                                }
                            });
                    });
                }
                None => {
                    ui.label(format!("Auto-empty: unknown ({})", auto_empty.raw));
                }
            },
            None => {
                ui.label(RichText::new("Auto-empty: …").color(TEXT_WEAK));
            }
        });
    }

    fn maintenance_tab(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, Some("Wear parts"), |ui| {
            if self.data.parts.is_empty() {
                ui.label(RichText::new("…").color(TEXT_WEAK));
            }
            for part in &self.data.parts {
                match part.percent() {
                    Some(percent) => theme::meter(ui, Id::unique(("part", &part.name)), &part.name, percent),
                    None => {
                        ui.label(format!("{}: {}", part.name, part.raw));
                    }
                }
            }
        });

        ui.add_space(14.0);
        theme::card(ui, Some("Totals"), |ui| match &self.data.stats {
            Some(stats) => {
                ui.columns(3, |columns| {
                    for (column, (value, label)) in columns.iter_mut().zip([
                        (&stats.total_cleans, "cleans"),
                        (&stats.total_area, "total area"),
                        (&stats.total_time, "total time"),
                    ]) {
                        column.label(RichText::new(value).size(26.0).strong().color(ACCENT));
                        column.label(RichText::new(label).color(TEXT_WEAK));
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "Last clean: area {}, time {} · units as reported by the robot",
                        stats.last_area, stats.last_time
                    ))
                    .color(TEXT_WEAK),
                );
            }
            None => {
                ui.label(RichText::new("…").color(TEXT_WEAK));
            }
        });

        ui.add_space(14.0);
        theme::card(ui, Some("Connection"), |ui| {
            match &self.data.network {
                Some(network) => {
                    theme::row(ui, "Wi-Fi network", |ui| {
                        ui.label(&network.ssid);
                    });
                    theme::row(ui, "IP address", |ui| {
                        ui.label(&network.ip);
                    });
                    theme::row(ui, "Signal", |ui| {
                        ui.label(&network.rssi);
                    });
                }
                None => {
                    ui.label(RichText::new("…").color(TEXT_WEAK));
                }
            }
            if let Some(firmware) = &self.data.firmware {
                ui.separator();
                theme::row(ui, "Firmware", |ui| {
                    ui.label(&firmware.firmware);
                });
                theme::row(ui, "Hardware", |ui| {
                    ui.label(&firmware.hardware);
                });
                theme::row(ui, "Software", |ui| {
                    ui.label(&firmware.software);
                });
            }
        });
    }

    fn rooms_tab(&mut self, ui: &mut egui::Ui, out: &mut Vec<Cmd>) {
        theme::card(ui, Some("Rooms on the robot's map"), |ui| {
            if self.data.rooms.is_empty() {
                ui.label(RichText::new("No rooms found (the robot needs a saved map).").color(TEXT_WEAK));
            }
            theme::row(ui, "Clean with", |ui| {
                egui::ComboBox::from_id_salt("room-mode").selected_text(work_mode_name(self.room_mode.value())).show_ui(ui, |ui| {
                    for mode in WorkMode::ALL {
                        ui.selectable_value(&mut self.room_mode, mode, work_mode_name(mode.value()));
                    }
                });
            });
            let rooms = self.data.rooms.clone();
            // Rooms that no longer exist on the map drop out of the selection
            self.selected_rooms.retain(|id| rooms.iter().any(|room| room.id == *id));
            for room in &rooms {
                let mut ticked = self.selected_rooms.contains(&room.id);
                theme::row(ui, &format!("{} · {}", room.id, room.label()), |ui| {
                    if ui.checkbox(&mut ticked, "").changed() {
                        self.confirm_rooms = false;
                    }
                });
                if ticked && !self.selected_rooms.contains(&room.id) {
                    self.selected_rooms.push(room.id);
                } else if !ticked {
                    self.selected_rooms.retain(|id| *id != room.id);
                }
            }
            ui.add_space(8.0);
            let chosen: Vec<(i64, String)> =
                rooms.iter().filter(|r| self.selected_rooms.contains(&r.id)).map(|r| (r.id, r.label().to_string())).collect();
            if chosen.is_empty() {
                ui.label(RichText::new("Tick the rooms to clean.").color(TEXT_WEAK));
            } else if self.confirm_rooms {
                ui.label(format!("Clean {}?", chosen.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")));
                match theme::button_grid(ui, &[("Yes, start", ACCENT, true), ("Cancel", Color32::from_rgb(70, 78, 96), true)]) {
                    Some(0) => {
                        out.push(Cmd::CleanRooms(chosen, self.room_mode));
                        self.confirm_rooms = false;
                    }
                    Some(_) => self.confirm_rooms = false,
                    None => {}
                }
            } else if theme::action(ui, &format!("Clean {} room{}", chosen.len(), if chosen.len() == 1 { "" } else { "s" }), ACCENT).clicked() {
                self.confirm_rooms = true;
            }
            ui.add_space(6.0);
            ui.label(RichText::new("The mode above is set just before the clean, because the robot resets it after a stop.").color(TEXT_WEAK));
        });
    }

    /// A message that slides up, then fades after a few seconds.
    fn toast(&self, ctx: &egui::Context) {
        let Some((ok, text)) = &self.notice else { return };
        let visible = self.notice_at.elapsed() < TOAST_TIME;
        let alpha = ctx.animate_bool_with_time(Id::unique("toast"), visible, 0.25);
        if alpha <= 0.0 {
            return;
        }
        let color = if *ok { OK } else { BAD };
        egui::Area::new(Id::unique("toast-area"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -20.0 + (1.0 - alpha) * 16.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                Frame::NONE
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, color))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::symmetric(18, 12))
                    .shadow(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) })
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.horizontal(|ui| {
                            theme::pulse_dot(ui, color, false);
                            ui.label(text);
                        });
                    });
            });
    }
}

impl eframe::App for DeebotApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();

        let logged_in = matches!(self.screen, Screen::Main);
        let status_every = if self.tab == Tab::Map { Duration::from_secs(8) } else { AUTO_REFRESH };
        if logged_in && self.pending == 0 && self.last_status.elapsed() >= status_every {
            self.cmd(Cmd::RefreshStatus);
        }
        if logged_in && self.tab == Tab::Map {
            let every = if self.is_cleaning() { LIVE_WHILE_CLEANING } else { LIVE_WHILE_IDLE };
            if self.last_live.elapsed() >= every {
                self.last_live = Instant::now();
                self.cmd(Cmd::RefreshLive { trace_len: self.data.trace.len() });
            }
        }
        // Wake up regularly so the auto-refresh timer is checked even when nothing else happens
        ui.ctx().request_repaint_after(Duration::from_secs(1));

        let mut out: Vec<Cmd> = Vec::new();

        // Keep clear of the status bar and gesture bar on a phone
        let (inset_top, inset_bottom) = platform::safe_insets(ui.ctx());

        if logged_in {
            egui::Panel::top("top")
                .frame(Frame::NONE.fill(BG).inner_margin(Margin {
                    left: 18,
                    right: 18,
                    top: (10.0 + inset_top) as i8,
                    bottom: 4,
                }))
                .show(ui, |ui| {
                    // Wraps onto a second line on narrow screens instead of running off the edge
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Deebot").size(22.0).strong().color(ACCENT));
                        theme::chip(ui, &self.robot_name);
                        if ui.add_enabled(self.pending == 0, egui::Button::new("Refresh")).clicked() {
                            out.push(Cmd::RefreshAll);
                        }
                        if ui.button("Sign out").clicked() {
                            self.screen = Screen::Login;
                            self.data = Data::default();
                            out.push(Cmd::Logout);
                        }
                    });
                    // The tabs fill the width without running past the screen edge
                    let labels: Vec<&str> = TABS.iter().map(|(_, label)| *label).collect();
                    let selected = TABS.iter().position(|(tab, _)| *tab == self.tab).unwrap_or(0);
                    if let Some(index) = theme::tab_row(ui, &labels, selected) {
                        self.tab = TABS[index].0;
                    }
                    theme::busy_bar(ui, self.pending > 0);
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG).inner_margin(Margin {
                left: 18,
                right: 18,
                // Without the header, the screen's own top edge needs the status-bar gap
                top: (14.0 + if logged_in { 0.0 } else { inset_top }) as i8,
                bottom: (14.0 + inset_bottom) as i8,
            }))
            .show(ui, |ui| match self.screen {
                Screen::Starting => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(90.0);
                        ui.spinner();
                        ui.label(RichText::new("Connecting to your robot…").color(TEXT_WEAK));
                    });
                }
                Screen::Login => self.login_screen(ui),
                Screen::Main => {
                    // Each tab fades and slides in when you switch to it
                    let ctx = ui.ctx().clone();
                    for (tab, _) in TABS {
                        if tab != self.tab {
                            ctx.animate_bool_with_time(Id::unique(("tab-fade", tab as u8)), false, 0.0);
                        }
                    }
                    let fade = ctx.animate_bool_with_time(Id::unique(("tab-fade", self.tab as u8)), true, 0.3);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_opacity(fade);
                        ui.add_space((1.0 - fade) * 16.0);
                        match self.tab {
                            Tab::Control => self.control_tab(ui, &mut out),
                            Tab::Map => self.map_tab(ui),
                            Tab::Settings => self.settings_tab(ui, &mut out),
                            Tab::Maintenance => self.maintenance_tab(ui),
                            Tab::Rooms => self.rooms_tab(ui, &mut out),
                        }
                    });
                }
            });

        self.toast(&ui.ctx().clone());

        for cmd in out {
            self.cmd(cmd);
        }
    }
}
