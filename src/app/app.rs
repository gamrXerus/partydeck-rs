use dialog::DialogBox;
use std::thread::sleep;

use super::config::*;
use crate::handler::*;
use crate::input::*;
use crate::instance::*;
use crate::launch::launch_game;
use crate::monitor::Monitor;
use crate::paths::*;
use crate::util::*;

use eframe::egui::{self, Key};

#[derive(Eq, PartialEq)]
pub enum MenuPage {
    Home,
    Settings,
    Profiles,
    EditHandler,
    Game,
    Instances,
}

#[derive(Eq, PartialEq)]
pub enum SettingsPage {
    General,
    Gamescope,
}

pub struct PartyApp {
    pub needs_update: bool,
    pub options: PartyConfig,
    pub cur_page: MenuPage,
    pub settings_page: SettingsPage,
    pub infotext: String,

    pub monitors: Vec<Monitor>,
    pub input_devices: Vec<InputDevice>,
    pub instances: Vec<Instance>,
    pub instance_add_dev: Option<usize>,
    pub handlers: Vec<Handler>,
    pub selected_handler: usize,
    pub handler_edit: Option<Handler>,
    pub profiles: Vec<String>,

    pub loading_msg: Option<String>,
    pub loading_since: Option<std::time::Instant>,
    #[allow(dead_code)]
    pub task: Option<std::thread::JoinHandle<()>>,
}

macro_rules! cur_handler {
    ($self:expr) => {
        &$self.handlers[$self.selected_handler]
    };
}

impl PartyApp {
    pub fn new(monitors: Vec<Monitor>) -> Self {
        let options = load_cfg();
        let input_devices = scan_input_devices(&options.pad_filter_type);
        Self {
            needs_update: check_for_partydeck_update(),
            options,
            cur_page: MenuPage::Home,
            settings_page: SettingsPage::General,
            infotext: String::new(),
            monitors,
            input_devices,
            instances: Vec::new(),
            instance_add_dev: None,
            handlers: scan_handlers(),
            selected_handler: 0,
            handler_edit: None,
            profiles: scan_profiles(false),
            loading_msg: None,
            loading_since: None,
            task: None,
        }
    }
}

impl eframe::App for PartyApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if !raw_input.focused || self.task.is_some() {
            return;
        }
        match self.cur_page {
            MenuPage::Instances => self.handle_devices_instance_menu(),
            _ => self.handle_gamepad_gui(raw_input),
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("menu_nav_panel").show(ctx, |ui| {
            if self.task.is_some() {
                ui.disable();
            }
            self.display_panel_top(ui);
        });

        egui::SidePanel::left("games_panel")
            .resizable(false)
            .exact_width(200.0)
            .show(ctx, |ui| {
                if self.task.is_some() {
                    ui.disable();
                }
                self.display_panel_left(ui);
            });

        if self.cur_page == MenuPage::Instances {
            egui::SidePanel::right("devices_panel")
                .resizable(false)
                .exact_width(180.0)
                .show(ctx, |ui| {
                    if self.task.is_some() {
                        ui.disable();
                    }
                    self.display_panel_right(ui, ctx);
                });
        }

        if (self.cur_page != MenuPage::Home) && (self.cur_page != MenuPage::Instances) {
            self.display_panel_bottom(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.task.is_some() {
                ui.disable();
            }
            match self.cur_page {
                MenuPage::Home => self.display_page_main(ui),
                MenuPage::Settings => self.display_page_settings(ui),
                MenuPage::Profiles => self.display_page_profiles(ui),
                MenuPage::EditHandler => self.display_page_edit_handler(ui),
                MenuPage::Game => self.display_page_game(ui),
                MenuPage::Instances => self.display_page_instances(ui),
            }
        });

        if let Some(handle) = self.task.take() {
            if handle.is_finished() {
                let _ = handle.join();
                self.loading_since = None;
                self.loading_msg = None;
            } else {
                self.task = Some(handle);
            }
        }
        if let Some(start) = self.loading_since {
            if start.elapsed() > std::time::Duration::from_secs(60) {
                // Give up waiting after one minute
                self.loading_msg = Some("Operation timed out".to_string());
            }
        }
        if let Some(msg) = &self.loading_msg {
            egui::Area::new("loading".into())
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 192))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add(egui::widgets::Spinner::new().size(40.0));
                                ui.add_space(8.0);
                                ui.label(msg);
                            });
                        });
                });
        }
        if ctx.input(|input| input.focused) {
            ctx.request_repaint_after(std::time::Duration::from_millis(33)); // 30 fps
        }
    }
}

impl PartyApp {
    pub fn spawn_task<F>(&mut self, msg: &str, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.loading_msg = Some(msg.to_string());
        self.loading_since = Some(std::time::Instant::now());
        self.task = Some(std::thread::spawn(f));
    }

    pub fn add_handler(&mut self) -> Result<(), String> {
        if let Some(uid) = dialog::Input::new("Enter ID for new handler (must be alphanumeric):")
            .title("New Handler")
            .show()
            .expect("Could not display dialog box")
        {
            if uid.is_empty() {
                return Err("ID cannot be empty".to_string());
            } else if !uid.chars().all(char::is_alphanumeric) {
                return Err("ID must be alphanumeric".to_string());
            } else if PATH_PARTY.join("handlers").join(&uid).exists() {
                return Err(format!("Handler with ID '{}' already exists", uid));
            } else {
                self.handler_edit = Some(Handler::new_from_uid(&uid));
                self.cur_page = MenuPage::EditHandler;
                return Ok(());
            }
        }

        Err("Error opening dialog box".to_string())
    }

    fn handle_gamepad_gui(&mut self, raw_input: &mut egui::RawInput) {
        let mut key: Option<egui::Key> = None;
        for pad in &mut self.input_devices {
            if !pad.enabled() {
                continue;
            }
            match pad.poll() {
                Some(PadButton::ABtn) => key = Some(Key::Enter),
                Some(PadButton::BBtn) => self.cur_page = MenuPage::Home,
                Some(PadButton::XBtn) => {
                    self.profiles = scan_profiles(false);
                    self.cur_page = MenuPage::Profiles;
                }
                Some(PadButton::YBtn) => self.cur_page = MenuPage::Settings,
                Some(PadButton::SelectBtn) => key = Some(Key::Tab),
                Some(PadButton::StartBtn) => {
                    if self.cur_page == MenuPage::Game {
                        self.instances.clear();
                        self.profiles = scan_profiles(true);
                        self.instance_add_dev = None;
                        self.cur_page = MenuPage::Instances;
                    }
                }
                Some(PadButton::Up) => key = Some(Key::ArrowUp),
                Some(PadButton::Down) => key = Some(Key::ArrowDown),
                Some(PadButton::Left) => key = Some(Key::ArrowLeft),
                Some(PadButton::Right) => key = Some(Key::ArrowRight),
                Some(_) => {}
                None => {}
            }
        }

        if let Some(key) = key {
            raw_input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
        }
    }

    fn handle_devices_instance_menu(&mut self) {
        let mut i = 0;
        while i < self.input_devices.len() {
            if !self.input_devices[i].enabled() {
                i += 1;
                continue;
            }
            match self.input_devices[i].poll() {
                Some(PadButton::ABtn) | Some(PadButton::ZKey) | Some(PadButton::RightClick) => {
                    if self.input_devices[i].device_type() != DeviceType::Gamepad
                        && !self.options.kbm_support
                    {
                        continue;
                    }
                    if !self.options.allow_multiple_instances_on_same_device
                        && self.is_device_in_any_instance(i)
                    {
                        continue;
                    }
                    // Prevent same keyboard/mouse device in multiple instances due to current custom gamescope limitations
                    // TODO: Remove this when custom gamescope supports the same keyboard/mouse device for multiple instances
                    if self.input_devices[i].device_type() != DeviceType::Gamepad
                        && self.is_device_in_any_instance(i)
                    {
                        continue;
                    }

                    match self.instance_add_dev {
                        Some(inst) => {
                            // Add the device in the instance only if it's not already there
                            if !self.is_device_in_instance(inst, i) {
                                self.instance_add_dev = None;
                                self.instances[inst].devices.push(i);
                            } else {
                                continue;
                            }
                        }
                        None => {
                            self.instances.push(Instance {
                                devices: vec![i],
                                profname: String::new(),
                                profselection: 0,
                                monitor: 0,
                                width: 0,
                                height: 0,
                            });
                        }
                    }
                }
                Some(PadButton::BBtn) | Some(PadButton::XKey) => {
                    if self.instance_add_dev != None {
                        self.instance_add_dev = None;
                    } else if self.is_device_in_any_instance(i) {
                        self.remove_device(i);
                    } else if self.instances.len() < 1 {
                        self.cur_page = MenuPage::Game;
                    }
                }
                Some(PadButton::YBtn) | Some(PadButton::AKey) => {
                    if self.instance_add_dev == None {
                        if let Some((instance, _)) = self.find_device_in_instance(i) {
                            self.instance_add_dev = Some(instance);
                        }
                    }
                }
                Some(PadButton::StartBtn) => {
                    if self.instances.len() > 0 && self.is_device_in_any_instance(i) {
                        self.prepare_game_launch();
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn is_device_in_any_instance(&self, dev: usize) -> bool {
        for instance in &self.instances {
            if instance.devices.contains(&dev) {
                return true;
            }
        }
        false
    }

    fn is_device_in_instance(&self, instance_index: usize, dev: usize) -> bool {
        if self.instances[instance_index].devices.contains(&dev) {
            return true;
        }
        false
    }

    fn find_device_in_instance(&mut self, dev: usize) -> Option<(usize, usize)> {
        for (i, instance) in self.instances.iter().enumerate() {
            for (d, device) in instance.devices.iter().enumerate() {
                if device == &dev {
                    return Some((i, d));
                }
            }
        }
        None
    }

    fn find_device_in_instance_from_end(&mut self, dev: usize) -> Option<(usize, usize)> {
        for (i, instance) in self.instances.iter().enumerate().rev() {
            for (d, device) in instance.devices.iter().enumerate() {
                if device == &dev {
                    return Some((i, d));
                }
            }
        }
        None
    }

    pub fn remove_device(&mut self, dev: usize) {
        if let Some((instance_index, device_index)) = self.find_device_in_instance_from_end(dev) {
            self.instances[instance_index].devices.remove(device_index);
            if self.instances[instance_index].devices.is_empty() {
                self.instances.remove(instance_index);
            }
        }
    }

    pub fn remove_device_instance(&mut self, instance_index: usize, dev: usize) {
        let device_index = self.instances[instance_index]
            .devices
            .iter()
            .position(|device| device == &dev);

        if let Some(d) = device_index {
            self.instances[instance_index].devices.remove(d);

            if self.instances[instance_index].devices.is_empty() {
                self.instances.remove(instance_index);
            }
        }
    }

    pub fn prepare_game_launch(&mut self) {
        if self.options.gamescope_sdl_backend {
            set_instance_resolutions_multimonitor(
                &mut self.instances,
                &self.monitors,
                &self.options,
            );
        } else {
            set_instance_resolutions(&mut self.instances, &self.monitors[0], &self.options);
        }
        set_instance_names(&mut self.instances, &self.profiles);

        let handler = cur_handler!(self).to_owned();
        let instances = self.instances.clone();
        let dev_infos: Vec<DeviceInfo> = self.input_devices.iter().map(|p| p.info()).collect();

        let cfg = self.options.clone();
        let _ = save_cfg(&cfg);

        self.cur_page = MenuPage::Home;
        self.spawn_task(
            "Launching...\n\nDon't press any buttons or move any analog sticks or mice.",
            move || {
                sleep(std::time::Duration::from_secs(2));
                if let Err(err) = launch_game(&handler, &dev_infos, &instances, &cfg) {
                    println!("[partydeck] Error: {}", err);
                    msg("Launch Error", &format!("{err}"));
                }
            },
        );
    }
}
