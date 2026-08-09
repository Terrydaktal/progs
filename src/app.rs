use crate::dependency_graph::{
    build_forward_dependency_graph, build_reverse_dependency_graph, DependencyGraph,
    DependencyGraphNode, DependencyGraphNodeKind,
};
use crate::models::{AppItem, BinaryInfo, InstallOrigin, InstallRole, ScanResult};
use crate::scanner::scan_system;
use crate::search::FuzzySearchRanker;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver};

pub struct ProgramManagerApp {
    apps: Vec<AppItem>,
    provides_map: HashMap<String, String>,
    selected_index: Option<usize>,
    search_query: String,
    search_ranker: FuzzySearchRanker,
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
    chk_cargo: bool,
    chk_opt: bool,
    chk_deps: bool,
    active_tab: usize,
    dependency_view: RelationshipView,
    used_by_view: RelationshipView,
    graph_zoom: f32,
    app_scale: f32,
    show_settings_window: bool,
    rx: Receiver<ScanResult>,
    is_loading: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RelationshipView {
    Tree,
    Graph,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum GraphTraversal {
    Dependencies,
    UsedBy,
}

const GRAPH_NODE_LIMIT: usize = 120;
const GRAPH_NODE_WIDTH: f32 = 260.0;
const GRAPH_NODE_HEIGHT: f32 = 68.0;
const GRAPH_COLUMN_GAP: f32 = 110.0;
const GRAPH_ROW_GAP: f32 = 28.0;
const GRAPH_PADDING: f32 = 24.0;
const GRAPH_HEADER_HEIGHT: f32 = 48.0;
const GRAPH_MIN_ZOOM: f32 = 0.5;
const GRAPH_MAX_ZOOM: f32 = 2.0;
const GRAPH_ZOOM_STEP: f32 = 1.2;

#[derive(Clone, Copy, Eq, PartialEq)]
enum GraphPortSide {
    Left,
    Right,
}

struct DependencyBranch<'a> {
    parent_pkg: &'a str,
    depth: usize,
    max_depth: usize,
    indent_level: usize,
    path: &'a [String],
}

fn badge_color(app: &AppItem) -> egui::Color32 {
    // Dependency status is intentionally a color distinction for Pacman
    // packages: they retain the [PAC] origin badge but use the legacy grey
    // dependency color.
    if app.origin == InstallOrigin::Pacman && app.install_role == InstallRole::Dependency {
        return egui::Color32::from_rgb(180, 180, 180);
    }

    match app.display_badge() {
        "PAC" => egui::Color32::from_rgb(16, 185, 129), // Pure Emerald Green
        "DEV" => egui::Color32::from_rgb(251, 113, 133), // Bright Neon Coral
        "FRK" => egui::Color32::from_rgb(13, 242, 177), // Mint Teal (Modified Git Fork!)
        "CLO" => egui::Color32::from_rgb(96, 165, 250), // Sky Slate Blue (Unmodified Cloned Repo!)
        "UNC" => egui::Color32::from_rgb(113, 113, 122), // Steel Gray (Unclassified Executable!)
        "BRK" => egui::Color32::from_rgb(82, 82, 91),   // Dark Gray (Broken Executable / Symlink!)
        "BIN" => egui::Color32::from_rgb(239, 68, 68),  // Crimson Red
        "SCR" => egui::Color32::from_rgb(249, 115, 22), // Tangerine Orange
        "OPT" => egui::Color32::from_rgb(250, 204, 21), // Canary Yellow
        "NPM" => egui::Color32::from_rgb(59, 130, 246), // Royal Blue
        "CAR" => egui::Color32::from_rgb(198, 91, 50),  // Cargo Rust
        "CST" => egui::Color32::from_rgb(99, 102, 241), // Indigo
        "AUR" => egui::Color32::from_rgb(217, 70, 239), // Vibrant Magenta
        "UV" => egui::Color32::from_rgb(168, 85, 247),  // Electric Purple
        _ => egui::Color32::from_rgb(180, 180, 180),    // Neutral Silver
    }
}

fn version_suffix(version: &str) -> String {
    if version.is_empty() {
        String::new()
    } else {
        format!(" ({version})")
    }
}

fn classified_display_text(app: &AppItem, name: &str) -> String {
    let capability_suffix = app.capability_suffix();
    let suffix = if capability_suffix.is_empty() {
        String::new()
    } else {
        format!(" {capability_suffix}")
    };
    format!(
        "[{}] {}{}{}",
        app.display_badge(),
        name,
        version_suffix(&app.version),
        suffix
    )
}

fn app_display_text(app: &AppItem) -> String {
    classified_display_text(app, &app.name)
}

fn executable_display_text(app: &AppItem, binary: &BinaryInfo) -> String {
    let capability_suffix = app.capability_suffix();
    let suffix = if capability_suffix.is_empty() {
        String::new()
    } else {
        format!(" {capability_suffix}")
    };
    let location = if binary.is_symlink && !binary.target.is_empty() {
        format!("{} -> {}", binary.path, binary.target)
    } else {
        binary.path.clone()
    };
    format!(
        "⚡ [{}] {}{}{} — {}",
        app.display_badge(),
        binary.name,
        version_suffix(&binary.version),
        suffix,
        location
    )
}

fn dependency_root_display_text(app: &AppItem) -> String {
    if app.is_one_to_one_standalone_tool() {
        executable_display_text(app, &app.binaries[0])
    } else {
        format!("📦 {}", app_display_text(app))
    }
}

fn apply_graph_zoom_delta(current: f32, delta: f32) -> f32 {
    (current * delta).clamp(GRAPH_MIN_ZOOM, GRAPH_MAX_ZOOM)
}

fn graph_zoom_scroll_adjustment(
    pointer_offset: egui::Vec2,
    old_zoom: f32,
    new_zoom: f32,
) -> egui::Vec2 {
    pointer_offset * (new_zoom / old_zoom - 1.0)
}

fn dependency_column_title(level: usize, has_provided_tools: bool) -> String {
    if has_provided_tools && level == 1 {
        return "PROVIDED TOOLS".to_string();
    }
    let dependency_level = level.saturating_sub(usize::from(has_provided_tools));
    if dependency_level == 1 {
        "DIRECT DEPENDENCIES".to_string()
    } else {
        format!(
            "TRANSITIVE DEPENDENCY LEVEL {}",
            dependency_level.saturating_sub(1)
        )
    }
}

fn dependency_sharing_status(required_by_count: usize) -> String {
    if required_by_count <= 1 {
        "Exclusive".to_string()
    } else {
        format!("Shared by {required_by_count} apps")
    }
}

fn dependent_column_title(level: usize) -> String {
    if level == 1 {
        "DIRECT DEPENDENTS".to_string()
    } else {
        format!("TRANSITIVE DEPENDENT LEVEL {}", level.saturating_sub(1))
    }
}

fn used_by_package_status(is_explicit: bool, required_by_count: usize) -> String {
    if is_explicit {
        "Explicitly installed root".to_string()
    } else if required_by_count == 0 {
        "Orphaned dependency".to_string()
    } else {
        let package_label = if required_by_count == 1 {
            "package"
        } else {
            "packages"
        };
        format!("Required by {required_by_count} installed {package_label}")
    }
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
            search_ranker: FuzzySearchRanker::default(),
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
            chk_cargo: true,
            chk_opt: true,
            chk_deps: true,
            active_tab: 0, // Default to 📦 Dependencies!
            dependency_view: RelationshipView::Tree,
            used_by_view: RelationshipView::Tree,
            graph_zoom: 1.0,
            app_scale: 1.2,
            show_settings_window: false,
            rx,
            is_loading: true,
        }
    }

    fn filter_app(&self, app: &AppItem) -> bool {
        if app.install_role == InstallRole::Dependency && !self.chk_deps {
            return false;
        }
        if app.state.dev && !self.chk_dev {
            return false;
        }
        if app.state.fork && !self.chk_forks {
            return false;
        }
        if app.state.cloned && !self.chk_clo {
            return false;
        }
        if app.state.unclassified && !self.chk_unc {
            return false;
        }
        if app.state.broken && !self.chk_brk {
            return false;
        }
        if (app.state.binary || app.origin == InstallOrigin::Uv) && !self.chk_bin {
            return false;
        }
        if app.state.script && !self.chk_scr {
            return false;
        }
        if app.origin == InstallOrigin::Npm && !self.chk_npm {
            return false;
        }
        if app.origin == InstallOrigin::Cargo && !self.chk_cargo {
            return false;
        }
        if app.state.opt && !self.chk_opt {
            return false;
        }

        if app.origin == InstallOrigin::Pacman && !self.chk_pacman {
            return false;
        }
        if app.origin == InstallOrigin::Aur && !self.chk_paru {
            return false;
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
            && self.chk_cargo
            && self.chk_opt
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
        self.chk_cargo = target;
        self.chk_opt = target;
        self.chk_deps = target;
    }

    fn calculate_max_left_width(&self, ctx: &egui::Context, app_indices: &[usize]) -> f32 {
        let mut max_w = 320.0f32;
        let font_id = egui::FontId::proportional(13.0);

        for &app_index in app_indices {
            let text = app_display_text(&self.apps[app_index]);
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
        max_w.min(580.0) // Clamp maximum width so right panel always has generous room
    }

    fn selectable_tree_prefix(ui: &mut egui::Ui, prefix: &str) {
        if !prefix.is_empty() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(prefix)
                        .monospace()
                        .color(egui::Color32::from_rgb(125, 132, 145)),
                )
                .selectable(true),
            );
        }
    }

    fn selectable_tree_leaf(ui: &mut egui::Ui, prefix: &str, text: impl Into<egui::WidgetText>) {
        ui.horizontal(|ui| {
            Self::selectable_tree_prefix(ui, prefix);
            let toggle_size = egui::vec2(ui.spacing().indent, ui.spacing().icon_width);
            let _ = ui.allocate_space(toggle_size);
            ui.add(egui::Label::new(text).selectable(true).extend());
        });
    }

    fn selectable_tree_collapsing_header<BodyReturn>(
        ui: &mut egui::Ui,
        id_source: impl std::hash::Hash,
        default_open: bool,
        prefix: &str,
        text: impl Into<egui::WidgetText>,
        add_body: impl FnOnce(&mut egui::Ui) -> BodyReturn,
    ) {
        let id = ui.make_persistent_id(id_source);
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            default_open,
        );
        ui.horizontal(|ui| {
            Self::selectable_tree_prefix(ui, prefix);
            state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
            ui.add(egui::Label::new(text).selectable(true));
        });
        state.show_body_unindented(ui, add_body);
    }

    fn dependency_tree_indent(level: usize) -> String {
        "    ".repeat(level)
    }

    fn render_dependency_children(
        &self,
        ui: &mut egui::Ui,
        app: &AppItem,
        max_depth: usize,
        indent_level: usize,
        path: &[String],
    ) {
        for dep in &app.depends_on {
            self.render_dep_tree_node(
                ui,
                dep,
                DependencyBranch {
                    parent_pkg: &app.name,
                    depth: 0,
                    max_depth,
                    indent_level,
                    path,
                },
            );
        }
    }

    fn render_dependency_tree(&self, ui: &mut egui::Ui, app: &AppItem, max_depth: usize) {
        let root_label = egui::RichText::new(dependency_root_display_text(app))
            .color(badge_color(app))
            .strong();
        let flatten_standalone_tool = app.is_one_to_one_standalone_tool();

        if app.binaries.is_empty() && app.depends_on.is_empty() {
            Self::selectable_tree_leaf(ui, "", root_label);
            Self::selectable_tree_leaf(
                ui,
                &Self::dependency_tree_indent(1),
                egui::RichText::new("No package dependencies.")
                    .color(egui::Color32::from_rgb(185, 190, 200)),
            );
            return;
        }

        Self::selectable_tree_collapsing_header(
            ui,
            ("dependency_root", &app.name),
            true,
            "",
            root_label,
            |ui| {
                if flatten_standalone_tool {
                    let dependency_indent = Self::dependency_tree_indent(1);
                    if app.depends_on.is_empty() {
                        Self::selectable_tree_leaf(
                            ui,
                            &dependency_indent,
                            egui::RichText::new("No package dependencies.")
                                .color(egui::Color32::from_rgb(185, 190, 200)),
                        );
                    } else {
                        let path = vec![app.name.clone()];
                        self.render_dependency_children(ui, app, max_depth, 1, &path);
                    }
                    return;
                }

                if app.binaries.is_empty() {
                    let path = vec![app.name.clone()];
                    self.render_dependency_children(ui, app, max_depth, 1, &path);
                    return;
                }

                let executable_indent = Self::dependency_tree_indent(1);
                let dependency_indent = Self::dependency_tree_indent(2);
                for binary in &app.binaries {
                    let executable_label =
                        egui::RichText::new(executable_display_text(app, binary))
                            .color(badge_color(app))
                            .strong();
                    let path = vec![app.name.clone(), binary.path.clone()];

                    Self::selectable_tree_collapsing_header(
                        ui,
                        ("dependency_executable", &binary.path),
                        true,
                        &executable_indent,
                        executable_label,
                        |ui| {
                            if app.depends_on.is_empty() {
                                Self::selectable_tree_leaf(
                                    ui,
                                    &dependency_indent,
                                    egui::RichText::new("No package dependencies.")
                                        .color(egui::Color32::from_rgb(185, 190, 200)),
                                );
                            } else {
                                self.render_dependency_children(ui, app, max_depth, 2, &path);
                            }
                        },
                    );
                }
            },
        );
    }

    fn render_dep_tree_node(
        &self,
        ui: &mut egui::Ui,
        dep_name: &str,
        branch: DependencyBranch<'_>,
    ) {
        if branch.depth >= branch.max_depth {
            return;
        }

        let real_pkg = self
            .provides_map
            .get(dep_name)
            .cloned()
            .unwrap_or_else(|| dep_name.to_string());

        let app_lookup = self.apps.iter().find(|a| a.name == real_pkg);
        let package_color = app_lookup
            .map(badge_color)
            .unwrap_or_else(|| egui::Color32::from_rgb(180, 180, 180));
        let req_users: HashSet<String> = if let Some(a) = app_lookup {
            a.required_by.clone()
        } else {
            HashSet::new()
        };

        let mut other_users: Vec<&String> = req_users
            .iter()
            .filter(|u| *u != branch.parent_pkg)
            .collect();
        other_users.sort_unstable();
        let is_exclusive = other_users.is_empty();

        let total_sharing_apps = req_users.len();

        let status_summary = if is_exclusive {
            "Exclusive (Will be uninstalled with -Rns)".to_string()
        } else {
            let u_str = other_users
                .iter()
                .take(3)
                .map(|s| s.as_str())
                .collect::<Vec<&str>>()
                .join(", ");
            format!("Shared by {} apps ({})", total_sharing_apps, u_str)
        };

        let mut job = egui::text::LayoutJob::default();
        let package_name = if real_pkg != dep_name {
            format!("{} [via {}]", dep_name, real_pkg)
        } else {
            dep_name.to_string()
        };
        let package_text = app_lookup
            .map(|dependency| classified_display_text(dependency, &package_name))
            .unwrap_or(package_name);

        // Every package segment uses the same classification color as the side panel.
        job.append(
            &format!("{package_text}  "),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(14.0),
                color: package_color,
                ..Default::default()
            },
        );

        job.append(
            &format!("—  {}", status_summary),
            0.0,
            egui::TextFormat {
                color: package_color,
                font_id: egui::FontId::proportional(13.0),
                ..Default::default()
            },
        );

        let is_cycle = branch.path.iter().any(|package| package == &real_pkg);
        let sub_deps = if is_cycle {
            Vec::new()
        } else {
            app_lookup.map(|a| a.depends_on.clone()).unwrap_or_default()
        };
        let indent = Self::dependency_tree_indent(branch.indent_level);

        if sub_deps.is_empty() || branch.depth + 1 >= branch.max_depth {
            Self::selectable_tree_leaf(ui, &indent, job);
        } else {
            let mut current_path = branch.path.to_vec();
            current_path.push(real_pkg.clone());
            let node_id = current_path.join("\u{1f}");

            Self::selectable_tree_collapsing_header(
                ui,
                ("dependency_node", node_id),
                false,
                &indent,
                job,
                |ui| {
                    for sub_dep in &sub_deps {
                        self.render_dep_tree_node(
                            ui,
                            sub_dep,
                            DependencyBranch {
                                parent_pkg: &real_pkg,
                                depth: branch.depth + 1,
                                max_depth: branch.max_depth,
                                indent_level: branch.indent_level + 1,
                                path: &current_path,
                            },
                        );
                    }
                },
            );
        }
    }

    fn render_reverse_dependency_tree(&self, ui: &mut egui::Ui, app: &AppItem) {
        let root_status = if app.install_role.is_explicit() {
            "  •  EXPLICIT ROOT"
        } else {
            ""
        };
        let root_label =
            egui::RichText::new(format!("📦 {}{}", app_display_text(app), root_status))
                .color(badge_color(app))
                .strong();
        let mut users: Vec<&String> = app.required_by.iter().collect();
        users.sort_unstable();

        if users.is_empty() {
            Self::selectable_tree_leaf(ui, "", root_label);
            let status = if app.install_role.is_explicit() {
                "This package is itself an explicitly installed root."
            } else {
                "No installed package requires this dependency; it is orphaned."
            };
            Self::selectable_tree_leaf(
                ui,
                &Self::dependency_tree_indent(1),
                egui::RichText::new(status).color(egui::Color32::from_rgb(185, 190, 200)),
            );
            return;
        }

        Self::selectable_tree_collapsing_header(
            ui,
            ("reverse_dependency_root", &app.name),
            true,
            "",
            root_label,
            |ui| {
                let path = vec![app.name.clone()];
                for user in users {
                    self.render_reverse_dependency_node(ui, user, 1, &path);
                }
            },
        );
    }

    fn render_relationship_graph(
        &mut self,
        ui: &mut egui::Ui,
        root_index: usize,
        traversal: GraphTraversal,
    ) -> Option<usize> {
        let graph = match traversal {
            GraphTraversal::Dependencies => build_forward_dependency_graph(
                &self.apps,
                &self.provides_map,
                root_index,
                GRAPH_NODE_LIMIT,
            ),
            GraphTraversal::UsedBy => {
                build_reverse_dependency_graph(&self.apps, root_index, GRAPH_NODE_LIMIT)
            }
        };
        let layers = graph.ordered_layers();
        let max_level = layers.len().saturating_sub(1);
        let has_provided_tools = graph
            .nodes
            .iter()
            .any(|node| matches!(node.kind, DependencyGraphNodeKind::ProvidedTool { .. }));

        let largest_layer = layers.iter().map(Vec::len).max().unwrap_or(1);
        let base_graph_width = GRAPH_PADDING * 2.0
            + (max_level + 1) as f32 * GRAPH_NODE_WIDTH
            + max_level as f32 * GRAPH_COLUMN_GAP;
        let base_graph_height = (GRAPH_HEADER_HEIGHT
            + GRAPH_PADDING * 2.0
            + largest_layer as f32 * GRAPH_NODE_HEIGHT
            + largest_layer.saturating_sub(1) as f32 * GRAPH_ROW_GAP)
            .max(260.0);
        let available_width = ui.available_width();
        let old_zoom = self.graph_zoom;
        let old_canvas_size = egui::vec2(
            (base_graph_width * old_zoom).max(available_width),
            base_graph_height * old_zoom,
        );
        let canvas_origin = ui.next_widget_position();
        let pointer_position = ui.input(|input| input.pointer.hover_pos());
        let pointer_over_graph = pointer_position.is_some_and(|pointer| {
            egui::Rect::from_min_size(canvas_origin, old_canvas_size).contains(pointer)
                && ui.clip_rect().contains(pointer)
        });
        if pointer_over_graph {
            let zoom_delta = ui.input(|input| input.zoom_delta());
            let new_zoom = apply_graph_zoom_delta(old_zoom, zoom_delta);
            if new_zoom != old_zoom {
                if let Some(pointer) = pointer_position {
                    let pointer_offset = pointer - canvas_origin;
                    let scroll_adjustment =
                        graph_zoom_scroll_adjustment(pointer_offset, old_zoom, new_zoom);
                    ui.scroll_with_delta(-scroll_adjustment);
                }
                self.graph_zoom = new_zoom;
                ui.ctx().request_repaint();
            }
        }

        let zoom = self.graph_zoom;
        let node_width = GRAPH_NODE_WIDTH * zoom;
        let node_height = GRAPH_NODE_HEIGHT * zoom;
        let column_gap = GRAPH_COLUMN_GAP * zoom;
        let row_gap = GRAPH_ROW_GAP * zoom;
        let padding = GRAPH_PADDING * zoom;
        let header_height = GRAPH_HEADER_HEIGHT * zoom;
        let graph_width = base_graph_width * zoom;
        let graph_height = base_graph_height * zoom;
        let canvas_size = egui::vec2(graph_width.max(ui.available_width()), graph_height);
        let (canvas_rect, _) = ui.allocate_exact_size(canvas_size, egui::Sense::hover());
        let mut node_rects =
            vec![egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::ZERO); graph.nodes.len()];
        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, 6.0 * zoom, egui::Color32::from_rgb(20, 22, 27));

        for (level, layer) in layers.iter().enumerate() {
            let layer_height =
                layer.len() as f32 * node_height + layer.len().saturating_sub(1) as f32 * row_gap;
            let x = canvas_rect.left() + padding + level as f32 * (node_width + column_gap);
            let nodes_top = canvas_rect.top() + header_height;
            let nodes_height = graph_height - header_height;
            let first_y = nodes_top + (nodes_height - layer_height) / 2.0;
            let column_rect = egui::Rect::from_min_max(
                egui::pos2(x - 14.0 * zoom, canvas_rect.top() + 6.0 * zoom),
                egui::pos2(
                    x + node_width + 14.0 * zoom,
                    canvas_rect.bottom() - 6.0 * zoom,
                ),
            );
            let column_fill = if level % 2 == 0 {
                egui::Color32::from_rgb(27, 30, 36)
            } else {
                egui::Color32::from_rgb(24, 27, 33)
            };
            painter.rect_filled(column_rect, 6.0 * zoom, column_fill);
            painter.rect_stroke(
                column_rect,
                6.0 * zoom,
                egui::Stroke::new(zoom, egui::Color32::from_rgb(45, 49, 58)),
            );
            let is_terminal_column = level + 1 == layers.len()
                && layer
                    .iter()
                    .all(|&node_index| graph.is_aligned_terminal(node_index));
            let column_title = if level == 0 {
                "SELECTED PACKAGE".to_string()
            } else if is_terminal_column && traversal == GraphTraversal::Dependencies {
                format!("TERMINAL DEPENDENCIES · {}", layer.len())
            } else if is_terminal_column
                && layer
                    .iter()
                    .all(|&node_index| graph.nodes[node_index].is_explicit)
            {
                format!("TERMINAL INSTALL ROOTS · {}", layer.len())
            } else if is_terminal_column {
                format!("TERMINAL ENDPOINTS · {}", layer.len())
            } else {
                match traversal {
                    GraphTraversal::Dependencies => {
                        dependency_column_title(level, has_provided_tools)
                    }
                    GraphTraversal::UsedBy => dependent_column_title(level),
                }
            };
            painter.text(
                egui::pos2(x + node_width / 2.0, canvas_rect.top() + 20.0 * zoom),
                egui::Align2::CENTER_CENTER,
                column_title,
                egui::FontId::proportional(12.0 * zoom),
                egui::Color32::from_rgb(155, 162, 175),
            );

            for (row, &node_index) in layer.iter().enumerate() {
                let y = first_y + row as f32 * (node_height + row_gap);
                node_rects[node_index] = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(node_width, node_height),
                );
            }
        }

        let mut selected_index = None;
        let mut hovered_node = None;
        for (node_index, node) in graph.nodes.iter().enumerate() {
            let rect = node_rects[node_index];
            let id = ui.make_persistent_id(("dependency_graph_node", traversal, node_index));
            let response = ui.interact(rect, id, egui::Sense::click());
            let hovered = response.hovered();
            let clicked = response.clicked();
            let package_text = self.graph_node_display_text(node);
            let status = self.graph_node_status(&graph, node_index, root_index, traversal);
            if hovered {
                hovered_node = Some(node_index);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            response.on_hover_text(format!("{package_text}\n{status}"));
            if clicked {
                selected_index = node.package_index;
            }
        }

        let highlighted_edges = hovered_node
            .map(|node| graph.path_edges_through(node))
            .unwrap_or_default();
        let mut highlighted_nodes = HashSet::new();
        if let Some(node) = hovered_node {
            highlighted_nodes.insert(node);
            for &edge_index in &highlighted_edges {
                let (source, target) = graph.edges[edge_index];
                highlighted_nodes.insert(source);
                highlighted_nodes.insert(target);
            }
        }
        let visible_edges: Vec<(usize, (usize, usize))> =
            graph.edges.iter().copied().enumerate().collect();
        let visible_edge_pairs: Vec<(usize, usize)> =
            visible_edges.iter().map(|&(_, edge)| edge).collect();
        let edge_ports = Self::graph_edge_ports(&visible_edge_pairs, &node_rects);

        for highlighted_pass in [false, true] {
            for (visible_edge_index, &(edge_index, _)) in visible_edges.iter().enumerate() {
                let is_highlighted = highlighted_edges.contains(&edge_index);
                if is_highlighted != highlighted_pass
                    || (hovered_node.is_none() && highlighted_pass)
                {
                    continue;
                }
                let color = if is_highlighted {
                    egui::Color32::from_rgb(96, 165, 250)
                } else if hovered_node.is_some() {
                    egui::Color32::from_rgba_unmultiplied(110, 118, 132, 36)
                } else {
                    egui::Color32::from_rgba_unmultiplied(130, 140, 158, 105)
                };
                let (start, end) = edge_ports[visible_edge_index];
                Self::paint_graph_edge(
                    &painter,
                    start,
                    end,
                    color,
                    if is_highlighted {
                        2.8 * zoom
                    } else {
                        1.3 * zoom
                    },
                    zoom,
                );
            }
        }

        for (node_index, node) in graph.nodes.iter().enumerate() {
            let rect = node_rects[node_index];
            let is_hovered = hovered_node == Some(node_index);
            let is_related = hovered_node.is_none() || highlighted_nodes.contains(&node_index);
            let base_color = match node.kind {
                DependencyGraphNodeKind::Package => node
                    .package_index
                    .map(|index| badge_color(&self.apps[index]))
                    .unwrap_or_else(|| egui::Color32::from_rgb(150, 155, 165)),
                DependencyGraphNodeKind::ProvidedTool {
                    owner_package_index,
                    ..
                } => badge_color(&self.apps[owner_package_index]),
            };
            let node_color = if is_related {
                base_color
            } else {
                Self::color_with_alpha(base_color, 80)
            };
            let fill = if is_hovered {
                egui::Color32::from_rgb(48, 57, 70)
            } else if !is_related {
                egui::Color32::from_rgb(25, 27, 32)
            } else if node_index == 0 {
                egui::Color32::from_rgb(36, 43, 52)
            } else {
                egui::Color32::from_rgb(32, 35, 42)
            };
            painter.rect_filled(rect, 7.0 * zoom, fill);
            painter.rect_stroke(
                rect,
                7.0 * zoom,
                egui::Stroke::new(
                    if is_hovered {
                        3.0 * zoom
                    } else if node_index == 0 {
                        2.2 * zoom
                    } else {
                        1.4 * zoom
                    },
                    node_color,
                ),
            );

            let package_text = self.graph_node_display_text(node);
            let status = self.graph_node_status(&graph, node_index, root_index, traversal);
            let text_painter = painter.with_clip_rect(rect.shrink(8.0 * zoom));
            text_painter.text(
                rect.left_top() + egui::vec2(10.0, 10.0) * zoom,
                egui::Align2::LEFT_TOP,
                &package_text,
                egui::FontId::proportional(14.0 * zoom),
                node_color,
            );
            text_painter.text(
                rect.left_bottom() + egui::vec2(10.0, -10.0) * zoom,
                egui::Align2::LEFT_BOTTOM,
                &status,
                egui::FontId::proportional(12.0 * zoom),
                if is_related {
                    egui::Color32::from_rgb(190, 195, 205)
                } else {
                    egui::Color32::from_rgb(95, 99, 108)
                },
            );
        }

        if graph.truncated {
            let relationship = match traversal {
                GraphTraversal::Dependencies => "dependencies",
                GraphTraversal::UsedBy => "users",
            };
            ui.colored_label(
                egui::Color32::from_rgb(250, 204, 21),
                format!(
                    "Graph limited to {GRAPH_NODE_LIMIT} packages; additional {relationship} are hidden. Use the tree for a focused branch view."
                ),
            );
        }

        selected_index
    }

    fn graph_node_status(
        &self,
        graph: &DependencyGraph,
        node_index: usize,
        root_index: usize,
        traversal: GraphTraversal,
    ) -> String {
        let node = &graph.nodes[node_index];
        if let DependencyGraphNodeKind::ProvidedTool {
            owner_package_index,
            binary_index,
        } = node.kind
        {
            let owner = &self.apps[owner_package_index];
            let binary = &owner.binaries[binary_index];
            let location = if binary.is_symlink && !binary.target.is_empty() {
                format!("{} → {}", binary.path, binary.target)
            } else {
                binary.path.clone()
            };
            return format!("Provided by {} · {location}", owner.name);
        }
        if traversal == GraphTraversal::Dependencies {
            return self.forward_graph_node_status(graph, node_index, root_index);
        }
        if node_index == 0 {
            let direct_users = self.apps[root_index].required_by.len();
            if direct_users == 0 {
                if node.is_explicit {
                    "Selected package · explicit root".to_string()
                } else {
                    "Selected package · no installed users".to_string()
                }
            } else {
                let dependent_label = if direct_users == 1 {
                    "dependent"
                } else {
                    "dependents"
                };
                format!("Selected package · {direct_users} direct {dependent_label}")
            }
        } else if let Some(package_index) = node.package_index {
            let package = &self.apps[package_index];
            used_by_package_status(
                package.install_role.is_explicit(),
                package.required_by.len(),
            )
        } else {
            "Package metadata unavailable".to_string()
        }
    }

    fn forward_graph_node_status(
        &self,
        graph: &DependencyGraph,
        node_index: usize,
        root_index: usize,
    ) -> String {
        let node = &graph.nodes[node_index];
        if node_index == 0 {
            let provided_tools = if self.apps[root_index].is_one_to_one_standalone_tool() {
                0
            } else {
                self.apps[root_index].binaries.len()
            };
            let direct_dependencies = self.apps[root_index].depends_on.len();
            if provided_tools == 0 {
                format!("Selected package · {direct_dependencies} direct dependencies")
            } else {
                format!(
                    "Selected package · {provided_tools} provided tools · {direct_dependencies} direct dependencies"
                )
            }
        } else if let Some(package_index) = node.package_index {
            dependency_sharing_status(self.apps[package_index].required_by.len())
        } else {
            "Package metadata unavailable".to_string()
        }
    }

    fn graph_node_display_text(&self, node: &DependencyGraphNode) -> String {
        match node.kind {
            DependencyGraphNodeKind::Package => node.package_index.map_or_else(
                || node.name.clone(),
                |index| {
                    let app = &self.apps[index];
                    if app.is_one_to_one_standalone_tool() {
                        executable_display_text(app, &app.binaries[0])
                    } else {
                        app_display_text(app)
                    }
                },
            ),
            DependencyGraphNodeKind::ProvidedTool {
                owner_package_index,
                binary_index,
            } => executable_display_text(
                &self.apps[owner_package_index],
                &self.apps[owner_package_index].binaries[binary_index],
            ),
        }
    }

    fn render_graph_zoom_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Zoom:").strong());
            if ui
                .add_enabled(self.graph_zoom > GRAPH_MIN_ZOOM, egui::Button::new("−"))
                .on_hover_text("Zoom out")
                .clicked()
            {
                self.graph_zoom = apply_graph_zoom_delta(self.graph_zoom, 1.0 / GRAPH_ZOOM_STEP);
            }
            if ui
                .button(format!("{:.0}%", self.graph_zoom * 100.0))
                .on_hover_text("Reset graph zoom to 100%")
                .clicked()
            {
                self.graph_zoom = 1.0;
            }
            if ui
                .add_enabled(self.graph_zoom < GRAPH_MAX_ZOOM, egui::Button::new("+"))
                .on_hover_text("Zoom in")
                .clicked()
            {
                self.graph_zoom = apply_graph_zoom_delta(self.graph_zoom, GRAPH_ZOOM_STEP);
            }
            ui.weak("Ctrl+scroll or pinch");
        });
    }

    fn graph_edge_ports(
        edges: &[(usize, usize)],
        node_rects: &[egui::Rect],
    ) -> Vec<(egui::Pos2, egui::Pos2)> {
        let mut ports = edges
            .iter()
            .map(|&(source, target)| (node_rects[source].center(), node_rects[target].center()))
            .collect::<Vec<_>>();
        let mut outgoing_left = vec![Vec::new(); node_rects.len()];
        let mut outgoing_right = vec![Vec::new(); node_rects.len()];
        let mut incoming_left = vec![Vec::new(); node_rects.len()];
        let mut incoming_right = vec![Vec::new(); node_rects.len()];

        for (edge_index, &(source, target)) in edges.iter().enumerate() {
            if node_rects[target].center().x >= node_rects[source].center().x {
                outgoing_right[source].push(edge_index);
            } else {
                outgoing_left[source].push(edge_index);
            }
            if node_rects[source].center().x < node_rects[target].center().x {
                incoming_left[target].push(edge_index);
            } else {
                incoming_right[target].push(edge_index);
            }
        }

        for node_index in 0..node_rects.len() {
            Self::assign_graph_port_group(
                &mut outgoing_left[node_index],
                edges,
                node_rects,
                &mut ports,
                node_index,
                GraphPortSide::Left,
                true,
            );
            Self::assign_graph_port_group(
                &mut outgoing_right[node_index],
                edges,
                node_rects,
                &mut ports,
                node_index,
                GraphPortSide::Right,
                true,
            );
            Self::assign_graph_port_group(
                &mut incoming_left[node_index],
                edges,
                node_rects,
                &mut ports,
                node_index,
                GraphPortSide::Left,
                false,
            );
            Self::assign_graph_port_group(
                &mut incoming_right[node_index],
                edges,
                node_rects,
                &mut ports,
                node_index,
                GraphPortSide::Right,
                false,
            );
        }

        Self::separate_reciprocal_edge_ports(edges, node_rects, &mut ports);

        ports
    }

    fn separate_reciprocal_edge_ports(
        edges: &[(usize, usize)],
        node_rects: &[egui::Rect],
        ports: &mut [(egui::Pos2, egui::Pos2)],
    ) {
        let edge_by_pair: HashMap<(usize, usize), usize> = edges
            .iter()
            .copied()
            .enumerate()
            .map(|(edge_index, edge)| (edge, edge_index))
            .collect();
        for (edge_index, &(source, target)) in edges.iter().enumerate() {
            if source >= target {
                continue;
            }
            let Some(&reverse_edge_index) = edge_by_pair.get(&(target, source)) else {
                continue;
            };
            let offset = node_rects[source].height().min(node_rects[target].height()) * 0.13;
            ports[edge_index].0.y -= offset;
            ports[edge_index].1.y -= offset;
            ports[reverse_edge_index].0.y += offset;
            ports[reverse_edge_index].1.y += offset;
        }
    }

    fn assign_graph_port_group(
        edge_indices: &mut [usize],
        edges: &[(usize, usize)],
        node_rects: &[egui::Rect],
        ports: &mut [(egui::Pos2, egui::Pos2)],
        node_index: usize,
        side: GraphPortSide,
        is_source: bool,
    ) {
        edge_indices.sort_unstable_by(|left, right| {
            let left_peer = if is_source {
                edges[*left].1
            } else {
                edges[*left].0
            };
            let right_peer = if is_source {
                edges[*right].1
            } else {
                edges[*right].0
            };
            node_rects[left_peer]
                .center()
                .y
                .total_cmp(&node_rects[right_peer].center().y)
        });
        for (rank, &edge_index) in edge_indices.iter().enumerate() {
            let position =
                Self::graph_port_position(node_rects[node_index], side, rank, edge_indices.len());
            if is_source {
                ports[edge_index].0 = position;
            } else {
                ports[edge_index].1 = position;
            }
        }
    }

    fn graph_port_position(
        rect: egui::Rect,
        side: GraphPortSide,
        rank: usize,
        count: usize,
    ) -> egui::Pos2 {
        let inset = rect.height() * (10.0 / GRAPH_NODE_HEIGHT);
        let usable_height = rect.height() - inset * 2.0;
        let y = rect.top() + inset + usable_height * (rank + 1) as f32 / (count + 1) as f32;
        let x = match side {
            GraphPortSide::Left => rect.left(),
            GraphPortSide::Right => rect.right(),
        };
        egui::pos2(x, y)
    }

    fn color_with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }

    fn paint_graph_edge(
        painter: &egui::Painter,
        start: egui::Pos2,
        end: egui::Pos2,
        color: egui::Color32,
        width: f32,
        zoom: f32,
    ) {
        let horizontal_distance = (end.x - start.x).abs();
        let bend = (horizontal_distance * 0.45).max(GRAPH_COLUMN_GAP * zoom * 0.42);
        let (control_one, control_two) = if end.x > start.x {
            (start + egui::vec2(bend, 0.0), end - egui::vec2(bend, 0.0))
        } else if end.x < start.x {
            (start - egui::vec2(bend, 0.0), end + egui::vec2(bend, 0.0))
        } else {
            (
                start + egui::vec2(GRAPH_COLUMN_GAP * zoom * 0.55, 0.0),
                end + egui::vec2(GRAPH_COLUMN_GAP * zoom * 0.55, 0.0),
            )
        };
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [start, control_one, control_two, end],
            false,
            egui::Color32::TRANSPARENT,
            egui::Stroke::new(width, color),
        ));

        let direction = end - control_two;
        if direction.length_sq() <= f32::EPSILON {
            return;
        }
        let direction = direction.normalized();
        let normal = egui::vec2(-direction.y, direction.x);
        painter.add(egui::Shape::convex_polygon(
            vec![
                end,
                end - direction * 10.0 * zoom + normal * 4.5 * zoom,
                end - direction * 10.0 * zoom - normal * 4.5 * zoom,
            ],
            color,
            egui::Stroke::NONE,
        ));
    }

    fn render_reverse_dependency_node(
        &self,
        ui: &mut egui::Ui,
        package_name: &str,
        indent_level: usize,
        path: &[String],
    ) {
        let package = self.apps.iter().find(|app| app.name == package_name);
        let package_color = package
            .map(badge_color)
            .unwrap_or_else(|| egui::Color32::from_rgb(180, 180, 180));
        let is_cycle = path.iter().any(|ancestor| ancestor == package_name);
        let is_explicit_root = package.is_some_and(|app| app.install_role.is_explicit());
        let mut users: Vec<&String> = package
            .map(|app| app.required_by.iter().collect())
            .unwrap_or_default();
        users.sort_unstable();

        let status = if is_cycle {
            "Cycle already shown on this path".to_string()
        } else if is_explicit_root {
            "Explicitly installed root".to_string()
        } else if package.is_none() {
            "Installed package metadata unavailable".to_string()
        } else if users.is_empty() {
            "Orphaned dependency".to_string()
        } else {
            format!("Required by {} installed packages", users.len())
        };

        let mut job = egui::text::LayoutJob::default();
        let package_text = package
            .map(|app| classified_display_text(app, package_name))
            .unwrap_or_else(|| package_name.to_string());
        job.append(
            &format!("{package_text}  "),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(14.0),
                color: package_color,
                ..Default::default()
            },
        );
        job.append(
            &format!("—  {status}"),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(13.0),
                color: package_color,
                ..Default::default()
            },
        );

        let indent = Self::dependency_tree_indent(indent_level);
        if is_cycle || is_explicit_root || package.is_none() || users.is_empty() {
            Self::selectable_tree_leaf(ui, &indent, job);
            return;
        }

        let mut current_path = path.to_vec();
        current_path.push(package_name.to_string());
        let node_id = current_path.join("\u{1f}");
        Self::selectable_tree_collapsing_header(
            ui,
            ("reverse_dependency_node", node_id),
            false,
            &indent,
            job,
            |ui| {
                for user in users {
                    self.render_reverse_dependency_node(ui, user, indent_level + 1, &current_path);
                }
            },
        );
    }
}

impl eframe::App for ProgramManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.is_loading {
            if let Ok(res) = self.rx.try_recv() {
                self.apps = res.apps;
                self.provides_map = res.provides_map;
                self.search_ranker.rebuild(&self.apps);
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
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("🔍 Search programs, commands, Cargo, dev, npm tools..."),
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
                ui.checkbox(&mut self.chk_cargo, "Cargo");
                ui.checkbox(&mut self.chk_opt, "Opt");
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
                    ui.add(egui::Slider::new(&mut self.app_scale, 0.75..=2.25).text("Zoom Scale"));
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
                    ui.label(format!("Current Pixels per Point: {:.2}", self.app_scale));
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

        let ranked_indices = self
            .search_ranker
            .ranked_indices(&self.search_query)
            .to_vec();
        let filtered_indices: Vec<usize> = ranked_indices
            .into_iter()
            .filter(|&index| self.filter_app(&self.apps[index]))
            .collect();
        let left_width = self.calculate_max_left_width(ctx, &filtered_indices);

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
                        for &idx in &filtered_indices {
                            let app = &self.apps[idx];
                            let is_selected = self.selected_index == Some(idx);

                            let item_color = badge_color(app);

                            let item_text =
                                egui::RichText::new(app_display_text(app)).color(if is_selected {
                                    egui::Color32::WHITE
                                } else {
                                    item_color
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
                        "Origin: {}  •  Install role: {}",
                        app.origin.label(),
                        app.install_role.label()
                    ));
                    let state_tags = app.state.tag_summary();
                    if !state_tags.is_empty() {
                        ui.label(format!("State: {state_tags}"));
                    }
                    if app.version.is_empty() {
                        ui.label(format!("Size: {}", app.size));
                    } else {
                        ui.label(format!("Version: {}  •  Size: {}", app.version, app.size));
                    }
                    ui.label(format!(
                        "Capabilities: {}  •  Primary role: {}",
                        app.capabilities.tag_summary(),
                        app.capabilities.primary_role()
                    ));
                    ui.label(format!("Description: {}", app.desc));

                    // Tab Selector Buttons (0: Dependencies, 1: Used By, 2: Desktop, 3: Services, 4: Info)
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(self.active_tab == 0, "📦 Dependencies")
                            .clicked()
                        {
                            self.active_tab = 0;
                        }
                        if ui
                            .selectable_label(self.active_tab == 1, "🌳 Used By")
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
                            .selectable_label(
                                self.active_tab == 3,
                                format!("⚙️ Services ({})", app.services.len()),
                            )
                            .clicked()
                        {
                            self.active_tab = 3;
                        }
                        if ui
                            .selectable_label(self.active_tab == 4, "ℹ️ Info")
                            .clicked()
                        {
                            self.active_tab = 4;
                        }
                    });
                    ui.separator();

                    egui::ScrollArea::both()
                        .id_source("right_inspector_scroll_area")
                        .auto_shrink([false, false])
                        .animated(false)
                        .show(ui, |ui| {
                            match self.active_tab {
                                0 => {
                                    ui.style_mut().interaction.selectable_labels = true;
                                    ui.style_mut().interaction.multi_widget_text_select = true;
                                    ui.horizontal(|ui| {
                                        let title = match self.dependency_view {
                                            RelationshipView::Tree => "Package Dependency Tree:",
                                            RelationshipView::Graph => "Package Dependency Graph:",
                                        };
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(title).strong())
                                                .selectable(true),
                                        );
                                        ui.separator();
                                        ui.selectable_value(
                                            &mut self.dependency_view,
                                            RelationshipView::Tree,
                                            "🌳 Tree",
                                        );
                                        ui.selectable_value(
                                            &mut self.dependency_view,
                                            RelationshipView::Graph,
                                            "🕸 Graph",
                                        );
                                    });
                                    ui.separator();
                                    match self.dependency_view {
                                        RelationshipView::Tree => {
                                            self.render_dependency_tree(ui, &app, 3);
                                        }
                                        RelationshipView::Graph => {
                                            self.render_graph_zoom_controls(ui);
                                            ui.label(
                                                "Read left → right: direct and transitive requirements follow dependency order, while genuine leaf dependencies are aligned in the final column. Hover a node to isolate its complete path; click it to inspect the package.",
                                            );
                                            ui.add_space(6.0);
                                            if let Some(selected_index) = self
                                                .render_relationship_graph(
                                                    ui,
                                                    idx,
                                                    GraphTraversal::Dependencies,
                                                )
                                            {
                                                self.selected_index = Some(selected_index);
                                            }
                                        }
                                    }
                                }
                                1 => {
                                    ui.style_mut().interaction.selectable_labels = true;
                                    ui.style_mut().interaction.multi_widget_text_select = true;
                                    ui.horizontal(|ui| {
                                        let title = match self.used_by_view {
                                            RelationshipView::Tree => {
                                                "Reverse Dependency Tree to Explicit Roots:"
                                            }
                                            RelationshipView::Graph => {
                                                "Reverse Dependency Graph to Explicit Roots:"
                                            }
                                        };
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(title).strong())
                                                .selectable(true),
                                        );
                                        ui.separator();
                                        ui.selectable_value(
                                            &mut self.used_by_view,
                                            RelationshipView::Tree,
                                            "🌳 Tree",
                                        );
                                        ui.selectable_value(
                                            &mut self.used_by_view,
                                            RelationshipView::Graph,
                                            "🕸 Graph",
                                        );
                                    });
                                    ui.separator();
                                    match self.used_by_view {
                                        RelationshipView::Tree => {
                                            self.render_reverse_dependency_tree(ui, &app);
                                        }
                                        RelationshipView::Graph => {
                                            self.render_graph_zoom_controls(ui);
                                            ui.label(
                                                "Read left → right: intermediate packages follow dependency order, while every genuine endpoint is aligned in the final terminal-roots column. Hover a node to isolate its complete path; click it to inspect the package.",
                                            );
                                            ui.add_space(6.0);
                                            if let Some(selected_index) =
                                                self.render_relationship_graph(
                                                    ui,
                                                    idx,
                                                    GraphTraversal::UsedBy,
                                                )
                                            {
                                                self.selected_index = Some(selected_index);
                                            }
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
                                            let visibility = if de.is_visible {
                                                ""
                                            } else {
                                                " (hidden/internal)"
                                            };
                                            let label_text = format!(
                                                "🖥️ {}{}  [{}]",
                                                de.name, visibility, de.file_path
                                            );
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
                                3 => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Service definitions for {} ({}):",
                                            app.name,
                                            app.services.len()
                                        ))
                                        .strong(),
                                    );
                                    ui.separator();
                                    if app.services.is_empty() {
                                        ui.label("No systemd or D-Bus service definition references this program.");
                                    } else {
                                        for service in &app.services {
                                            let state = if service.broken {
                                                "broken"
                                            } else if service.running == Some(true) {
                                                "running"
                                            } else if service.running == Some(false) {
                                                "stopped"
                                            } else {
                                                "runtime state unavailable"
                                            };
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "⚙️ {} — {} · {} · {}",
                                                    service.name,
                                                    service.kind.label(),
                                                    service.scope,
                                                    state
                                                ))
                                                .strong(),
                                            );
                                            ui.label(format!(
                                                "   ↳ Enabled: {}",
                                                if service.enabled { "yes" } else { "no" }
                                            ));
                                            ui.label(format!("   ↳ Definition: {}", service.file_path));
                                            if !service.command.is_empty() {
                                                ui.label(format!("   ↳ ExecStart: {}", service.command));
                                            }
                                            if !service.activators.is_empty() {
                                                ui.label(format!(
                                                    "   ↳ Activation: {}",
                                                    service.activators.join(", ")
                                                ));
                                            }
                                            ui.add_space(6.0);
                                        }
                                    }
                                }
                                _ => {
                                    ui.label(format!("• Name: {}", app.name));
                                    ui.label(format!("• Origin: {}", app.origin.label()));
                                    ui.label(format!("• Install role: {}", app.install_role.label()));
                                    let state_tags = app.state.tag_summary();
                                    if !state_tags.is_empty() {
                                        ui.label(format!("• State: {state_tags}"));
                                    }
                                    if !app.version.is_empty() {
                                        ui.label(format!("• Version: {}", app.version));
                                    }
                                    ui.label(format!("• Install Date: {}", app.install_date));
                                    ui.label(format!("• Installed Size: {}", app.size));
                                    ui.label(format!("• License: {}", app.licenses));
                                    ui.label(format!("• URL: {}", app.url));
                                    ui.label(format!(
                                        "• Capabilities: {}",
                                        app.capabilities.tag_summary()
                                    ));
                                    ui.label(format!(
                                        "• Primary role: {}",
                                        app.capabilities.primary_role()
                                    ));

                                    if !app.capabilities.cli_commands.is_empty() {
                                        ui.label(format!(
                                            "• User-facing commands: {}",
                                            app.capabilities.cli_commands.join(", ")
                                        ));
                                    }

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
                                                "• {}{} {}",
                                                b.name,
                                                version_suffix(&b.version),
                                                loc_bracket
                                            ));
                                        }
                                    }
                                }
                            }
                        });

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("📋 Copy Info").clicked() {
                            let version_line = if app.version.is_empty() {
                                String::new()
                            } else {
                                format!("Version: {}\n", app.version)
                            };
                            let text = format!(
                                "Application: {}\nOrigin: {}\nInstall role: {}\nState: {}\n{}Capabilities: {}\nPrimary role: {}\nSize: {}\nDesc: {}\n",
                                app.name,
                                app.origin.label(),
                                app.install_role.label(),
                                app.state.tag_summary(),
                                version_line,
                                app.capabilities.tag_summary(),
                                app.capabilities.primary_role(),
                                app.size,
                                app.desc
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

#[cfg(test)]
mod tests {
    use super::{
        apply_graph_zoom_delta, dependency_column_title, dependency_root_display_text,
        dependency_sharing_status, dependent_column_title, graph_zoom_scroll_adjustment,
        used_by_package_status, ProgramManagerApp, GRAPH_MAX_ZOOM, GRAPH_MIN_ZOOM,
    };
    use crate::models::{
        AppItem, BinaryInfo, InstallOrigin, InstallRole, PackageCapabilities, ProgramState,
    };
    use eframe::egui;
    use std::collections::HashSet;

    #[test]
    fn dependency_tree_indent_uses_four_spaces_per_level() {
        assert_eq!(ProgramManagerApp::dependency_tree_indent(0), "");
        assert_eq!(ProgramManagerApp::dependency_tree_indent(1), "    ");
        assert_eq!(ProgramManagerApp::dependency_tree_indent(3), "            ");
    }

    #[test]
    fn one_to_one_standalone_dependency_roots_use_one_executable_label() {
        let app = AppItem {
            name: "chunk".to_string(),
            version: String::new(),
            origin: InstallOrigin::Local,
            install_role: InstallRole::Standalone,
            state: ProgramState {
                script: true,
                ..ProgramState::default()
            },
            size: "Local File".to_string(),
            install_date: String::new(),
            desc: String::new(),
            url: String::new(),
            licenses: String::new(),
            _owning_pkg: String::new(),
            binaries: vec![BinaryInfo {
                name: "chunk".to_string(),
                dir: "/home/lewis/.local/bin".to_string(),
                path: "/home/lewis/.local/bin/chunk".to_string(),
                is_symlink: false,
                target: String::new(),
                version: String::new(),
                _is_pacman_owned: false,
                _owning_pkg: String::new(),
            }],
            required_by: HashSet::new(),
            depends_on: Vec::new(),
            desktop_entries: Vec::new(),
            services: Vec::new(),
            capabilities: PackageCapabilities {
                has_cli: true,
                cli_commands: vec!["chunk".to_string()],
                ..PackageCapabilities::default()
            },
        };

        assert_eq!(
            dependency_root_display_text(&app),
            "⚡ [SCR] chunk (cli) — /home/lewis/.local/bin/chunk"
        );
    }

    #[test]
    fn dependency_columns_account_for_an_optional_provided_tools_layer() {
        assert_eq!(dependency_column_title(1, false), "DIRECT DEPENDENCIES");
        assert_eq!(
            dependency_column_title(2, false),
            "TRANSITIVE DEPENDENCY LEVEL 1"
        );
        assert_eq!(dependency_column_title(1, true), "PROVIDED TOOLS");
        assert_eq!(dependency_column_title(2, true), "DIRECT DEPENDENCIES");
        assert_eq!(
            dependency_column_title(4, true),
            "TRANSITIVE DEPENDENCY LEVEL 2"
        );
    }

    #[test]
    fn dependency_nodes_summarize_sharing_without_package_names() {
        assert_eq!(dependency_sharing_status(1), "Exclusive");
        assert_eq!(dependency_sharing_status(22), "Shared by 22 apps");
    }

    #[test]
    fn used_by_columns_use_direct_then_zero_based_transitive_labels() {
        assert_eq!(dependent_column_title(1), "DIRECT DEPENDENTS");
        assert_eq!(dependent_column_title(2), "TRANSITIVE DEPENDENT LEVEL 1");
        assert_eq!(dependent_column_title(4), "TRANSITIVE DEPENDENT LEVEL 3");
    }

    #[test]
    fn used_by_nodes_summarize_reverse_dependency_state() {
        assert_eq!(used_by_package_status(true, 3), "Explicitly installed root");
        assert_eq!(used_by_package_status(false, 0), "Orphaned dependency");
        assert_eq!(
            used_by_package_status(false, 1),
            "Required by 1 installed package"
        );
        assert_eq!(
            used_by_package_status(false, 22),
            "Required by 22 installed packages"
        );
    }

    #[test]
    fn graph_zoom_scales_and_clamps_to_usable_limits() {
        assert_eq!(apply_graph_zoom_delta(1.0, 1.2), 1.2);
        assert_eq!(apply_graph_zoom_delta(1.9, 2.0), GRAPH_MAX_ZOOM);
        assert_eq!(apply_graph_zoom_delta(0.6, 0.5), GRAPH_MIN_ZOOM);
    }

    #[test]
    fn graph_zoom_keeps_the_pointer_anchored_by_compensating_scroll() {
        assert_eq!(
            graph_zoom_scroll_adjustment(egui::vec2(120.0, 80.0), 1.0, 1.5),
            egui::vec2(60.0, 40.0)
        );
    }

    #[test]
    fn reciprocal_graph_edges_use_separate_loop_lanes() {
        let edges = [(0, 1), (1, 0)];
        let node_rects = [
            egui::Rect::from_min_size(egui::pos2(0.0, 20.0), egui::vec2(260.0, 68.0)),
            egui::Rect::from_min_size(egui::pos2(370.0, 20.0), egui::vec2(260.0, 68.0)),
        ];
        let ports = ProgramManagerApp::graph_edge_ports(&edges, &node_rects);

        assert!(ports[0].0.y < node_rects[0].center().y);
        assert!(ports[0].1.y < node_rects[1].center().y);
        assert!(ports[1].0.y > node_rects[1].center().y);
        assert!(ports[1].1.y > node_rects[0].center().y);
    }
}
