slint::include_modules!();

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// A small persisted list of (display name, real path) pairs - used for both
/// source ports and IWADs, since both are "browse for a file, remember it,
/// show it in a dropdown, allow removing it" in exactly the same shape.
struct NamedPaths {
    entries: Vec<(String, PathBuf)>,
    config_file: PathBuf,
}

impl NamedPaths {
    fn load(file_name: &str) -> Self {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("doom-launcher");
        let _ = std::fs::create_dir_all(&dir);
        let config_file = dir.join(file_name);

        let mut entries: Vec<(String, PathBuf)> = Vec::new();
        if let Ok(contents) = std::fs::read_to_string(&config_file) {
            for line in contents.lines() {
                if let Some((name, path_str)) = line.split_once('|') {
                    let path = PathBuf::from(path_str);
                    if !entries.iter().any(|(_, p)| p == &path) {
                        entries.push((name.to_string(), path));
                    }
                }
            }
        }

        let result = NamedPaths {
            entries,
            config_file,
        };
        result.save(); // rewrites the file, dropping any duplicates that had piled up
        result
    }

    fn save(&self) {
        let contents: String = self
            .entries
            .iter()
            .map(|(name, p)| format!("{}|{}\n", name, p.display()))
            .collect();
        let _ = std::fs::write(&self.config_file, contents);
    }

    /// Adds a path, deriving its display name from the filename, and persists.
    /// If the path is already present, reuses that entry instead of duplicating it.
    fn add(&mut self, path: PathBuf) {
        if self.entries.iter().any(|(_, p)| p == &path) {
            return;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        self.entries.push((name, path));
        self.save();
    }

    fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
            self.save();
        }
    }

    fn names(&self) -> Vec<SharedString> {
        self.entries
            .iter()
            .map(|(n, _)| SharedString::from(n.as_str()))
            .collect()
    }

    fn path_at(&self, index: usize) -> Option<&PathBuf> {
        self.entries.get(index).map(|(_, p)| p)
    }
}

/// A tiny key/value settings file, for single remembered values (like "which
/// folder did you last pick") that don't fit the list shape NamedPaths handles.
struct Settings {
    values: HashMap<String, String>,
    config_file: PathBuf,
}

impl Settings {
    fn load() -> Self {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("doom-launcher");
        let _ = std::fs::create_dir_all(&dir);
        let config_file = dir.join("settings.txt");

        let mut values = HashMap::new();
        if let Ok(contents) = std::fs::read_to_string(&config_file) {
            for line in contents.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    values.insert(key.to_string(), value.to_string());
                }
            }
        }
        Settings {
            values,
            config_file,
        }
    }

    fn get_path(&self, key: &str) -> Option<PathBuf> {
        self.values.get(key).map(PathBuf::from)
    }

    fn set_path(&mut self, key: &str, value: &PathBuf) {
        self.values
            .insert(key.to_string(), value.display().to_string());
        self.save();
    }

    fn get_list(&self, key: &str) -> Vec<String> {
        self.values
            .get(key)
            .map(|v| {
                v.split('|')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set_list(&mut self, key: &str, items: &[String]) {
        self.values.insert(key.to_string(), items.join("|"));
        self.save();
    }

    fn save(&self) {
        let contents: String = self
            .values
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect();
        let _ = std::fs::write(&self.config_file, contents);
    }
}

fn scan_wad_folder(folder: &PathBuf) -> Vec<ModItem> {
    let mut items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext = ext.to_lowercase();
                if ext == "wad" || ext == "pk3" || ext == "pk7" || ext == "zip" {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        items.push(ModItem {
                            name: SharedString::from(name),
                            path: SharedString::from(path.display().to_string()),
                            enabled: false,
                        });
                    }
                }
            }
        }
    }
    items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    items
}

/// Re-marks items whose name was in the persisted enabled list - used right
/// after a fresh scan, since scanning always starts every item as disabled.
fn apply_enabled(items: &mut [ModItem], enabled_names: &[String]) {
    for item in items.iter_mut() {
        if enabled_names.iter().any(|n| n == item.name.as_str()) {
            item.enabled = true;
        }
    }
}

/// Pushes the current port list to the main window's dropdown (and the
/// settings window's list, if it's open), clamping the selected index if
/// it just fell out of range (e.g. the selected port got removed).
fn refresh_ports(main_ui: &MainWindow, settings_win: Option<&SettingsWindow>, ports: &NamedPaths) {
    let names = ports.names();
    main_ui.set_available_ports(ModelRc::new(VecModel::from(names.clone())));

    let len = ports.entries.len();
    let current = main_ui.get_selected_port_index();
    if len == 0 {
        main_ui.set_selected_port_index(-1);
    } else if current < 0 || current as usize >= len {
        main_ui.set_selected_port_index(0);
    }

    if let Some(sw) = settings_win {
        sw.set_port_names(ModelRc::new(VecModel::from(names)));
    }
}

fn refresh_iwads(main_ui: &MainWindow, settings_win: Option<&SettingsWindow>, iwads: &NamedPaths) {
    let names = iwads.names();
    main_ui.set_available_iwads(ModelRc::new(VecModel::from(names.clone())));

    let len = iwads.entries.len();
    let current = main_ui.get_selected_iwad_index();
    if len == 0 {
        main_ui.set_selected_iwad_index(-1);
    } else if current < 0 || current as usize >= len {
        main_ui.set_selected_iwad_index(0);
    }

    if let Some(sw) = settings_win {
        sw.set_iwad_names(ModelRc::new(VecModel::from(names)));
    }
}

fn main() {
    let ui = MainWindow::new().unwrap();

    let source_ports = Rc::new(RefCell::new(NamedPaths::load("source_ports.txt")));
    let iwads = Rc::new(RefCell::new(NamedPaths::load("iwads.txt")));
    let settings = Rc::new(RefCell::new(Settings::load()));

    refresh_ports(&ui, None, &source_ports.borrow());
    refresh_iwads(&ui, None, &iwads.borrow());

    if let Some(folder) = settings.borrow().get_path("mods_folder") {
        let mut found = scan_wad_folder(&folder);
        apply_enabled(&mut found, &settings.borrow().get_list("enabled_mods"));
        ui.set_mods_folder(SharedString::from(folder.display().to_string()));
        ui.set_mods(ModelRc::new(VecModel::from(found)));
    }
    if let Some(folder) = settings.borrow().get_path("maps_folder") {
        let mut found = scan_wad_folder(&folder);
        apply_enabled(&mut found, &settings.borrow().get_list("enabled_maps"));
        ui.set_maps_folder(SharedString::from(folder.display().to_string()));
        ui.set_maps(ModelRc::new(VecModel::from(found)));
    }

    // Keeps the settings window alive across clicks so repeated "Settings"
    // presses reuse the same window instead of spawning new ones.
    let settings_window_handle: Rc<RefCell<Option<SettingsWindow>>> = Rc::new(RefCell::new(None));

    {
        let ui_weak = ui.as_weak();
        let source_ports = source_ports.clone();
        let iwads = iwads.clone();
        let settings = settings.clone();
        let settings_window_handle = settings_window_handle.clone();

        ui.on_settings_clicked(move || {
            let ui = ui_weak.upgrade().unwrap();

            if let Some(existing) = settings_window_handle.borrow().as_ref() {
                existing.set_mods_folder(ui.get_mods_folder());
                existing.set_maps_folder(ui.get_maps_folder());
                existing.show().unwrap();
                return;
            }

            let sw = SettingsWindow::new().unwrap();
            sw.set_mods_folder(ui.get_mods_folder());
            sw.set_maps_folder(ui.get_maps_folder());
            sw.set_port_names(ModelRc::new(VecModel::from(source_ports.borrow().names())));
            sw.set_iwad_names(ModelRc::new(VecModel::from(iwads.borrow().names())));

            {
                let ui_weak = ui.as_weak();
                let sw_weak = sw.as_weak();
                let settings = settings.clone();
                sw.on_choose_mods_folder_clicked(move || {
                    let ui = ui_weak.upgrade().unwrap();
                    let sw = sw_weak.upgrade().unwrap();
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        let mut found = scan_wad_folder(&folder);
                        apply_enabled(&mut found, &settings.borrow().get_list("enabled_mods"));
                        let folder_str = SharedString::from(folder.display().to_string());
                        ui.set_mods_folder(folder_str.clone());
                        ui.set_mods(ModelRc::new(VecModel::from(found)));
                        sw.set_mods_folder(folder_str);
                        settings.borrow_mut().set_path("mods_folder", &folder);
                    }
                });
            }

            {
                let ui_weak = ui.as_weak();
                let sw_weak = sw.as_weak();
                let settings = settings.clone();
                sw.on_choose_maps_folder_clicked(move || {
                    let ui = ui_weak.upgrade().unwrap();
                    let sw = sw_weak.upgrade().unwrap();
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        let mut found = scan_wad_folder(&folder);
                        apply_enabled(&mut found, &settings.borrow().get_list("enabled_maps"));
                        let folder_str = SharedString::from(folder.display().to_string());
                        ui.set_maps_folder(folder_str.clone());
                        ui.set_maps(ModelRc::new(VecModel::from(found)));
                        sw.set_maps_folder(folder_str);
                        settings.borrow_mut().set_path("maps_folder", &folder);
                    }
                });
            }

            {
                let ui_weak = ui.as_weak();
                let sw_weak = sw.as_weak();
                let source_ports = source_ports.clone();
                sw.on_add_port_clicked(move || {
                    let ui = ui_weak.upgrade().unwrap();
                    let sw = sw_weak.upgrade().unwrap();
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        source_ports.borrow_mut().add(path);
                        refresh_ports(&ui, Some(&sw), &source_ports.borrow());
                    }
                });
            }

            {
                let ui_weak = ui.as_weak();
                let sw_weak = sw.as_weak();
                let source_ports = source_ports.clone();
                sw.on_remove_port_clicked(move |index| {
                    let ui = ui_weak.upgrade().unwrap();
                    let sw = sw_weak.upgrade().unwrap();
                    source_ports.borrow_mut().remove(index as usize);
                    refresh_ports(&ui, Some(&sw), &source_ports.borrow());
                });
            }

            {
                let ui_weak = ui.as_weak();
                let sw_weak = sw.as_weak();
                let iwads = iwads.clone();
                sw.on_add_iwad_clicked(move || {
                    let ui = ui_weak.upgrade().unwrap();
                    let sw = sw_weak.upgrade().unwrap();
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        iwads.borrow_mut().add(path);
                        refresh_iwads(&ui, Some(&sw), &iwads.borrow());
                    }
                });
            }

            {
                let ui_weak = ui.as_weak();
                let sw_weak = sw.as_weak();
                let iwads = iwads.clone();
                sw.on_remove_iwad_clicked(move |index| {
                    let ui = ui_weak.upgrade().unwrap();
                    let sw = sw_weak.upgrade().unwrap();
                    iwads.borrow_mut().remove(index as usize);
                    refresh_iwads(&ui, Some(&sw), &iwads.borrow());
                });
            }

            sw.show().unwrap();
            *settings_window_handle.borrow_mut() = Some(sw);
        });
    }

    {
        let ui_weak = ui.as_weak();
        let settings = settings.clone();
        ui.on_mod_item_toggled(move |_index| {
            let ui = ui_weak.upgrade().unwrap();
            let enabled: Vec<String> = ui
                .get_mods()
                .iter()
                .filter(|m| m.enabled)
                .map(|m| m.name.to_string())
                .collect();
            settings.borrow_mut().set_list("enabled_mods", &enabled);
        });
    }

    {
        let ui_weak = ui.as_weak();
        let settings = settings.clone();
        ui.on_map_item_toggled(move |_index| {
            let ui = ui_weak.upgrade().unwrap();
            let enabled: Vec<String> = ui
                .get_maps()
                .iter()
                .filter(|m| m.enabled)
                .map(|m| m.name.to_string())
                .collect();
            settings.borrow_mut().set_list("enabled_maps", &enabled);
        });
    }

    {
        let ui_weak = ui.as_weak();
        let source_ports = source_ports.clone();
        let iwads = iwads.clone();
        ui.on_launch_clicked(move || {
            let ui = ui_weak.upgrade().unwrap();

            let port_index = ui.get_selected_port_index();
            let ports = source_ports.borrow();
            let exe_path = if port_index >= 0 {
                ports.path_at(port_index as usize)
            } else {
                None
            };
            let Some(exe_path) = exe_path else {
                eprintln!("No source port selected - add one in Settings first");
                return;
            };

            let iwad_index = ui.get_selected_iwad_index();
            let iwads_ref = iwads.borrow();
            let iwad_path = if iwad_index >= 0 {
                iwads_ref.path_at(iwad_index as usize)
            } else {
                None
            };

            let mods = ui.get_mods();
            let maps = ui.get_maps();
            let mut file_args: Vec<String> = mods
                .iter()
                .filter(|m| m.enabled)
                .map(|m| m.path.to_string())
                .collect();
            file_args.extend(
                maps.iter()
                    .filter(|m| m.enabled)
                    .map(|m| m.path.to_string()),
            );

            let mut command = std::process::Command::new(exe_path);
            if let Some(iwad) = iwad_path {
                command.arg("-iwad").arg(iwad);
            }
            if !file_args.is_empty() {
                command.arg("-file");
                command.args(&file_args);
            }

            println!("Launching: {:?}", command);

            if let Err(e) = command.spawn() {
                eprintln!("Failed to launch: {e}");
            }
        });
    }

    ui.run().unwrap();
}
