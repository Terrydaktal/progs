use crate::models::{AppItem, ScanResult};
use crate::scanner::scan_system;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver};

pub struct ProgramManagerApp {
    apps: Vec<AppItem>,
    provides_map: HashMap<String, String>,
    selected_index: Option<usize>,
    search_query: String,
    chk_pacman: bool,
    chk_paru: bool,
    chk_dev: bool,
    chk_forks: bool,
    chk_clo: bool,
    chk_unc: bool,
    chk_brk: bool,
    chk_bin: bool,
    chk_scr: bool,
    chk_npm: bool,
    chk_opt: bool,
    chk_sys: bool,
    chk_deps: bool,
    active_tab: usize,
    app_scale: f32,
    show_settings_window: bool,
    rx: Receiver<ScanResult>,
    is_loading: bool,
}

impl ProgramManagerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals.dark_mode = true;
        cc.egui_ctx.set_style(style);

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let res = scan_system();
            let _ = tx.send(res);
        });

        Self {
            apps: Vec::new(),
            provides_map: HashMap::new(),
            selected_index: None,
            search_query: String::new(),
            chk_pacman: true,
            chk_paru: true,
            chk_dev: true,
            chk_forks: true,
            chk_clo: true,
            chk_unc: true,
            chk_brk: true,
            chk_bin: true,
            chk_scr: true,
            chk_npm: true,
            chk_opt: true,
            chk_sys: true,
            chk_deps: true,
            active_tab: 0, // Default to 📦 Dependencies!
            app_scale: 1.2,
            show_settings_window: false,
            rx,
            is_loading: true,
        }
    }

    fn filter_app(&self, app: &AppItem) -> bool {
        let code = app.badge_code.as_str();
        let src = app.install_source.as_str();

        if code == "SYS" && !self.chk_sys {
            return false;
        }
        if code == "DEP" && !self.chk_deps {
            return false;
        }
        if code == "DEV" && !self.chk_dev {
            return false;
        }
        if code == "FRK" && !self.chk_forks {
            return false;
        }
        if code == "CLO" && !self.chk_clo {
            return false;
        }
        if (code == "UNC" || code == "CST") && !self.chk_unc {
            return false;
        }
        if code == "BRK" && !self.chk_brk {
            return false;
        }
        if (code == "BIN" || code == "UV") && !self.chk_bin {
            return false;
        }
        if code == "SCR" && !self.chk_scr {
            return false;
        }
        if code == "NPM" && !self.chk_npm {
            return false;
        }
        if code == "OPT" && !self.chk_opt {
            return false;
        }

        if src == "pacman"
            && ![
                "SYS", "DEP", "DEV", "FRK", "CLO", "UNC", "BRK", "OPT", "BIN", "SCR", "NPM", "UV",
                "CST",
            ]
            .contains(&code)
            && !self.chk_pacman
        {
            return false;
        }
        if src == "paru"
            && ![
                "SYS", "DEP", "DEV", "FRK", "CLO", "UNC", "BRK", "OPT", "BIN", "SCR", "NPM", "UV",
                "CST",
            ]
            .contains(&code)
            && !self.chk_paru
        {
            return false;
        }

        if !self.search_query.is_empty() {
            let q = self.search_query.to_lowercase();
            let match_name = app.name.to_lowercase().contains(&q);
            let match_ver = app.version.to_lowercase().contains(&q);
            let match_cat = app.category_label.to_lowercase().contains(&q);
            let match_bin = app.binaries.iter().any(|b| {
                b.name.to_lowercase().contains(&q) || b.target.to_lowercase().contains(&q)
            });
            let match_req = app.required_by.iter().any(|r| r.to_lowercase().contains(&q));
            if !(match_name || match_ver || match_cat || match_bin || match_req) {
                return false;
            }
        }

        true
    }

    fn are_all_filters_on(&self) -> bool {
        self.chk_pacman
            && self.chk_paru
            && self.chk_dev
            && self.chk_forks
            && self.chk_clo
            && self.chk_unc
            && self.chk_brk
            && self.chk_bin
            && self.chk_scr
            && self.chk_npm
            && self.chk_opt
            && self.chk_sys
            && self.chk_deps
    }

    fn toggle_all_filters(&mut self) {
        let target = !self.are_all_filters_on();
        self.chk_pacman = target;
        self.chk_paru = target;
        self.chk_dev = target;
        self.chk_forks = target;
        self.chk_clo = target;
        self.chk_unc = target;
        self.chk_brk = target;
        self.chk_bin = target;
        self.chk_scr = target;
        self.chk_npm = target;
        self.chk_opt = target;
        self.chk_sys = target;
        self.chk_deps = target;
    }

    fn calculate_max_left_width(&self, ctx: &egui::Context) -> f32 {
        let mut max_w = 320.0f32;
        let font_id = egui::FontId::proportional(13.0);

        for app in &self.apps {
            if self.filter_app(app) {
                let text = format!("[{}] {}  ({})", app.badge_code, app.name, app.version);
                let text_w = ctx.fonts(|f| {
                    f.layout_no_wrap(text, font_id.clone(), egui::Color32::WHITE)
                        .rect
                        .width()
                });
                let row_w = text_w + 42.0;
                if row_w > max_w {
                    max_w = row_w;
                }
            }
        }
        max_w.min(580.0) // Clamp maximum width so right panel always has generous room
    }

    fn render_dep_tree_node(
        &self,
        ui: &mut egui::Ui,
        parent_pkg: &str,
        dep_name: &str,
        depth: usize,
        max_depth: usize,
    ) {
        if depth >= max_depth {
            return;
        }

        let real_pkg = self
            .provides_map
            .get(dep_name)
            .cloned()
            .unwrap_or_else(|| dep_name.to_string());

        let app_lookup = self.apps.iter().find(|a| a.name == real_pkg);
        let req_users: HashSet<String> = if let Some(a) = app_lookup {
            a.required_by.clone()
        } else {
            HashSet::new()
        };

        let other_users: Vec<&String> = req_users.iter().filter(|u| *u != parent_pkg).collect();
        let is_exclusive = other_users.is_empty();

        let dep_ver = app_lookup.map(|a| a.version.as_str()).unwrap_or("");
        let ver_str = if !dep_ver.is_empty() {
            format!(" v{}", dep_ver)
        } else {
            "".to_string()
        };
        let prov_str = if real_pkg != dep_name {
            format!(" [via {}]", real_pkg)
        } else {
            "".to_string()
        };

        let color = if is_exclusive {
            egui::Color32::from_rgb(250, 204, 21) // Bright Yellow
        } else {
            egui::Color32::from_rgb(74, 222, 128) // Bright Green
        };

        let total_sharing_apps = req_users.len();

        let status_summary = if is_exclusive {
            "🟡 Exclusive (Will be uninstalled with -Rns)".to_string()
        } else {
            let u_str = other_users
                .iter()
                .take(3)
                .map(|s| s.as_str())
                .collect::<Vec<&str>>()
                .join(", ");
            format!("🟢 Shared by {} apps ({})", total_sharing_apps, u_str)
        };

        let mut job = egui::text::LayoutJob::default();

        // Package name, version, and provides in Red / Green
        job.append(
            &format!("{}{}{}  ", dep_name, ver_str, prov_str),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(14.0),
                color,
                ..Default::default()
            },
        );

        // Sharing summary in Soft Light Gray
        job.append(
            &format!("—  {}", status_summary),
            0.0,
            egui::TextFormat {
                color: egui::Color32::from_rgb(185, 190, 200),
                font_id: egui::FontId::proportional(13.0),
                ..Default::default()
            },
        );

        let sub_deps = app_lookup
            .map(|a| a.depends_on.clone())
            .unwrap_or_default();

        if sub_deps.is_empty() || depth + 1 >= max_depth {
            ui.label(job);
        } else {
            egui::CollapsingHeader::new(job)
                .default_open(depth == 0)
                .show(ui, |ui| {
                    for sub_dep in sub_deps {
                        self.render_dep_tree_node(ui, &real_pkg, &sub_dep, depth + 1, max_depth);
                    }
                });
        }
    }
}

impl eframe::App for ProgramManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.is_loading {
            if let Ok(res) = self.rx.try_recv() {
                self.apps = res.apps;
                self.provides_map = res.provides_map;
                self.is_loading = false;
            }
        }

        // Apply scale setting to egui context
        ctx.set_pixels_per_point(self.app_scale);

        // Top Single-Row Compact Header Panel (28px)
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("🛡️ progs")
                        .strong()
                        .color(egui::Color32::from_rgb(56, 189, 248)),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_query).hint_text(
                        "🔍 Search programs, commands, dev projects, npm tools...",
                    ),
                );

                let all_btn_label = if self.are_all_filters_on() {
                    "☐ None"
                } else {
                    "☑ All"
                };
                if ui.button(all_btn_label).clicked() {
                    self.toggle_all_filters();
                }

                ui.checkbox(&mut self.chk_pacman, "Pacman");
                ui.checkbox(&mut self.chk_paru, "Paru");
                ui.checkbox(&mut self.chk_dev, "Dev");
                ui.checkbox(&mut self.chk_forks, "Forks");
                ui.checkbox(&mut self.chk_clo, "Cloned");
                ui.checkbox(&mut self.chk_unc, "Unclassified");
                ui.checkbox(&mut self.chk_brk, "Broken");
                ui.checkbox(&mut self.chk_bin, "BIN");
                ui.checkbox(&mut self.chk_scr, "Script");
                ui.checkbox(&mut self.chk_npm, "NPM");
                ui.checkbox(&mut self.chk_opt, "Opt");
                ui.checkbox(&mut self.chk_sys, "Sys");
                ui.checkbox(&mut self.chk_deps, "Deps");

                if ui.button("🔄").clicked() {
                    self.is_loading = true;
                    let (tx, rx) = channel();
                    self.rx = rx;
                    std::thread::spawn(move || {
                        let res = scan_system();
                        let _ = tx.send(res);
                    });
                }

                if ui.button("⚙️ Settings").clicked() {
                    self.show_settings_window = !self.show_settings_window;
                }
            });
        });

        if self.show_settings_window {
            egui::Window::new("⚙️ Application Settings")
                .collapsible(false)
                .resizable(false)
                .default_size([420.0, 240.0])
                .show(ctx, |ui| {
                    ui.heading("Application Scale & UI Preferences");
                    ui.separator();
                    ui.label(egui::RichText::new("UI Zoom Scale (Pixels per Point):").strong());
                    ui.add(
                        egui::Slider::new(&mut self.app_scale, 0.75..=2.25).text("Zoom Scale"),
                    );
                    ui.separator();
                    ui.label("Quick Scale Presets:");
                    ui.horizontal(|ui| {
                        if ui.button("0.85x (Compact)").clicked() {
                            self.app_scale = 0.85;
                        }
                        if ui.button("1.00x (Default)").clicked() {
                            self.app_scale = 1.00;
                        }
                        if ui.button("1.25x (Large)").clicked() {
                            self.app_scale = 1.25;
                        }
                        if ui.button("1.50x (XL)").clicked() {
                            self.app_scale = 1.50;
                        }
                    });
                    ui.separator();
                    ui.label(format!(
                        "Current Pixels per Point: {:.2}",
                        self.app_scale
                    ));
                    ui.separator();
                    if ui.button("Close Settings").clicked() {
                        self.show_settings_window = false;
                    }
                });
        }

        if self.is_loading {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.heading(
                        "⚡ Loading Pacman Database & Filesystem Binaries in Parallel Rust...",
                    );
                });
            });
            return;
        }

        let left_width = self.calculate_max_left_width(ctx);

        let left_frame = egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin {
            left: 8.0,
            right: 0.0, // ZERO right margin! Scrollbar touches divider directly!
            top: 8.0,
            bottom: 8.0,
        });

        egui::SidePanel::left("left_program_list_panel")
            .frame(left_frame)
            .resizable(true)
            .default_width(left_width)
            .min_width(240.0)
            .max_width(600.0)
            .show(ctx, |ui| {
                ui.heading("Program List");
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_source("left_program_list_scroll_area")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let filtered_indices: Vec<usize> = self
                            .apps
                            .iter()
                            .enumerate()
                            .filter(|(_, a)| self.filter_app(a))
                            .map(|(i, _)| i)
                            .collect();

                        for &idx in &filtered_indices {
                            let app = &self.apps[idx];
                            let is_selected = self.selected_index == Some(idx);

                            let badge_color = match app.badge_code.as_str() {
                                "PAC" => egui::Color32::from_rgb(16, 185, 129), // Pure Emerald Green
                                "DEV" => egui::Color32::from_rgb(251, 113, 133), // Bright Neon Coral
                                "FRK" => egui::Color32::from_rgb(13, 242, 177), // Mint Teal (Modified Git Fork!)
                                "CLO" => egui::Color32::from_rgb(96, 165, 250), // Sky Slate Blue (Unmodified Cloned Repo!)
                                "UNC" => egui::Color32::from_rgb(113, 113, 122), // Steel Gray (Unclassified Executable!)
                                "BRK" => egui::Color32::from_rgb(82, 82, 91), // Dark Gray (Broken Executable / Symlink!)
                                "BIN" => egui::Color32::from_rgb(239, 68, 68), // Crimson Red
                                "SCR" => egui::Color32::from_rgb(249, 115, 22), // Tangerine Orange
                                "OPT" => egui::Color32::from_rgb(250, 204, 21), // Canary Yellow
                                "SYS" => egui::Color32::from_rgb(14, 165, 233), // Sky Blue
                                "NPM" => egui::Color32::from_rgb(59, 130, 246), // Royal Blue
                                "CST" => egui::Color32::from_rgb(99, 102, 241), // Indigo
                                "AUR" => egui::Color32::from_rgb(217, 70, 239), // Vibrant Magenta
                                "UV" => egui::Color32::from_rgb(168, 85, 247), // Electric Purple
                                "DEP" => egui::Color32::from_rgb(180, 180, 180), // Neutral Silver
                                _ => egui::Color32::from_rgb(180, 180, 180),
                            };

                            let item_text = egui::RichText::new(format!(
                                "[{}] {}  ({})",
                                app.badge_code, app.name, app.version
                            ))
                            .color(if is_selected {
                                egui::Color32::WHITE
                            } else {
                                badge_color
                            });

                            let item_response = ui.selectable_label(is_selected, item_text);

                            if item_response.clicked() {
                                self.selected_index = Some(idx);
                            }
                        }
                    });
            });

        let central_frame = egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin {
            left: 8.0,
            right: 0.0, // ZERO right margin! Right scrollbar touches right window edge directly!
            top: 8.0,
            bottom: 8.0,
        });

        egui::CentralPanel::default()
            .frame(central_frame)
            .show(ctx, |ui| {
                ui.heading("Program Inspector");
                ui.separator();

                if let Some(idx) = self.selected_index {
                    let app = self.apps[idx].clone();

                    ui.label(
                        egui::RichText::new(&app.name)
                            .heading()
                            .strong()
                            .color(egui::Color32::from_rgb(250, 204, 21)),
                    );
                    ui.label(format!(
                        "Classification: {} [{}]",
                        app.category_label, app.badge_code
                    ));
                    ui.label(format!("Version: {}  •  Size: {}", app.version, app.size));
                    ui.label(format!("Description: {}", app.desc));

                    // Tab Selector Buttons (0: Dependencies, 1: Used By, 2: Desktop Entries, 3: Info)
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(self.active_tab == 0, "📦 Dependencies")
                            .clicked()
                        {
                            self.active_tab = 0;
                        }
                        if ui
                            .selectable_label(self.active_tab == 1, "🔗 Used By")
                            .clicked()
                        {
                            self.active_tab = 1;
                        }
                        if ui
                            .selectable_label(
                                self.active_tab == 2,
                                format!("🖥️ Desktop Entries ({})", app.desktop_entries.len()),
                            )
                            .clicked()
                        {
                            self.active_tab = 2;
                        }
                        if ui
                            .selectable_label(self.active_tab == 3, "ℹ️ Info")
                            .clicked()
                        {
                            self.active_tab = 3;
                        }
                    });
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .id_source("right_inspector_scroll_area")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            match self.active_tab {
                                0 => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Executable Dependencies for {}:",
                                            app.name
                                        ))
                                        .strong(),
                                    );
                                    ui.separator();

                                    if app.binaries.is_empty() {
                                        if app.depends_on.is_empty() {
                                            ui.label("No dependencies required by this application.");
                                        } else {
                                            for dep in &app.depends_on {
                                                self.render_dep_tree_node(
                                                    ui, &app.name, dep, 0, 3,
                                                );
                                            }
                                        }
                                    } else {
                                        for b in &app.binaries {
                                            let loc_bracket =
                                                if b.is_symlink && !b.target.is_empty() {
                                                    format!("[{} -> {}]", b.path, b.target)
                                                } else {
                                                    format!("[{}]", b.path)
                                                };
                                            let exec_label = format!(
                                                "⚡ {} ({}) {}",
                                                b.name, b.version, loc_bracket
                                            );
                                            let exec_richtext = egui::RichText::new(&exec_label)
                                                .color(egui::Color32::from_rgb(56, 189, 248))
                                                .strong();

                                            if app.depends_on.is_empty() {
                                                ui.label(exec_richtext);
                                                ui.label(
                                                    "   ↳ No dependencies required by this executable.",
                                                );
                                            } else {
                                                egui::CollapsingHeader::new(exec_richtext)
                                                    .default_open(true)
                                                    .show(ui, |ui| {
                                                        for dep in &app.depends_on {
                                                            self.render_dep_tree_node(
                                                                ui, &app.name, dep, 0, 3,
                                                            );
                                                        }
                                                    });
                                            }
                                        }
                                    }
                                }
                                1 => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Required/Used By {} Applications:",
                                            app.required_by.len()
                                        ))
                                        .strong(),
                                    );
                                    if app.required_by.is_empty() {
                                        ui.label(
                                            "No other installed package lists this as a direct dependency.",
                                        );
                                    } else {
                                        for r in &app.required_by {
                                            ui.label(format!("• {}", r));
                                        }
                                    }
                                }
                                2 => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Desktop Launchers & Shortcuts for {} ({}):",
                                            app.name,
                                            app.desktop_entries.len()
                                        ))
                                        .strong(),
                                    );
                                    ui.separator();

                                    if app.desktop_entries.is_empty() {
                                        ui.label(
                                            "No .desktop launcher shortcut references this application.",
                                        );
                                    } else {
                                        for de in &app.desktop_entries {
                                            let label_text =
                                                format!("🖥️ {}  [{}]", de.name, de.file_path);
                                            ui.label(
                                                egui::RichText::new(label_text)
                                                    .color(egui::Color32::from_rgb(56, 189, 248))
                                                    .strong(),
                                            );
                                            ui.label(format!("   ↳ Exec: {}", de.exec));
                                            if !de.comment.is_empty() {
                                                ui.label(format!("   ↳ Comment: {}", de.comment));
                                            }
                                            if !de.icon.is_empty() {
                                                ui.label(format!("   ↳ Icon: {}", de.icon));
                                            }
                                            ui.add_space(6.0);
                                        }
                                    }
                                }
                                _ => {
                                    ui.label(format!("• Name: {}", app.name));
                                    ui.label(format!("• Category: {}", app.category_label));
                                    ui.label(format!("• Version: {}", app.version));
                                    ui.label(format!("• Install Date: {}", app.install_date));
                                    ui.label(format!("• Installed Size: {}", app.size));
                                    ui.label(format!("• License: {}", app.licenses));
                                    ui.label(format!("• URL: {}", app.url));

                                    if !app.binaries.is_empty() {
                                        ui.separator();
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Executables & Symlinks ({}):",
                                                app.binaries.len()
                                            ))
                                            .strong(),
                                        );
                                        for b in &app.binaries {
                                            let loc_bracket =
                                                if b.is_symlink && !b.target.is_empty() {
                                                    format!("[{} -> {}]", b.path, b.target)
                                                } else {
                                                    format!("[{}]", b.path)
                                                };
                                            ui.label(format!(
                                                "• {} ({}) {}",
                                                b.name, b.version, loc_bracket
                                            ));
                                        }
                                    }
                                }
                            }
                        });

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("📋 Copy Info").clicked() {
                            let text = format!(
                                "Application: {}\nCategory: {}\nVersion: {}\nSize: {}\nDesc: {}\n",
                                app.name, app.category_label, app.version, app.size, app.desc
                            );
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let _ = clipboard.set_text(text);
                            }
                        }
                        if ui.button("📁 Open Location").clicked() {
                            let target_dir = if let Some(b) = app.binaries.first() {
                                b.dir.clone()
                            } else {
                                "/usr/bin".to_string()
                            };
                            let _ = open::that(target_dir);
                        }
                    });
                } else {
                    ui.label("Select a program on the left to inspect.");
                    if self.active_tab == 4 {
                        ui.separator();
                        ui.label(
                            egui::RichText::new("⚙️ Application Settings")
                                .heading()
                                .strong()
                                .color(egui::Color32::from_rgb(56, 189, 248)),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new("Application UI Scaling / Zoom Factor:").strong(),
                        );
                        ui.add(
                            egui::Slider::new(&mut self.app_scale, 0.75..=2.25).text("Zoom Scale"),
                        );

                        ui.horizontal(|ui| {
                            ui.label("Quick Presets:");
                            if ui.button("0.85x (Compact)").clicked() {
                                self.app_scale = 0.85;
                            }
                            if ui.button("1.00x (Default)").clicked() {
                                self.app_scale = 1.00;
                            }
                            if ui.button("1.25x (Large)").clicked() {
                                self.app_scale = 1.25;
                            }
                            if ui.button("1.50x (XL)").clicked() {
                                self.app_scale = 1.50;
                            }
                        });
                    }
                }
            });
    }
}
