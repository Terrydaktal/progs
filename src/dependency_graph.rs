use crate::models::AppItem;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Eq, PartialEq)]
pub struct DependencyGraphNode {
    pub name: String,
    pub package_index: Option<usize>,
    pub kind: DependencyGraphNodeKind,
    pub dependency_level: usize,
    pub is_explicit: bool,
    pub has_hidden_successors: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyGraphNodeKind {
    Package,
    ProvidedTool {
        owner_package_index: usize,
        binary_index: usize,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub struct DependencyGraph {
    pub nodes: Vec<DependencyGraphNode>,
    pub edges: Vec<(usize, usize)>,
    pub truncated: bool,
    component_by_node: Vec<usize>,
    components: Vec<Vec<usize>>,
}

impl DependencyGraph {
    pub fn is_cycle_component(&self, component: usize) -> bool {
        self.components[component].len() > 1
            || self.edges.iter().any(|&(source, target)| {
                source == target && self.component_by_node[source] == component
            })
    }

    pub fn component_has_explicit_package(&self, component: usize) -> bool {
        self.components[component]
            .iter()
            .any(|&node_index| self.nodes[node_index].is_explicit)
    }

    pub fn is_terminal_component(&self, component: usize) -> bool {
        !self.components[component]
            .iter()
            .any(|&node_index| self.nodes[node_index].has_hidden_successors)
            && !self.edges.iter().any(|&(source, target)| {
                self.component_by_node[source] == component
                    && self.component_by_node[target] != component
            })
    }

    pub fn is_aligned_terminal(&self, node_index: usize) -> bool {
        if matches!(
            self.nodes[node_index].kind,
            DependencyGraphNodeKind::ProvidedTool { .. }
        ) {
            return false;
        }
        let component = self.component_by_node[node_index];
        component != self.component_by_node[0]
            && self.is_terminal_component(component)
            && (!self.is_cycle_component(component)
                || !self.component_has_explicit_package(component)
                || self.nodes[node_index].is_explicit)
    }

    pub fn ordered_layers(&self) -> Vec<Vec<usize>> {
        let terminal_nodes: HashSet<usize> = (1..self.nodes.len())
            .filter(|&node_index| self.is_aligned_terminal(node_index))
            .collect();
        let max_internal_level = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(node_index, _)| !terminal_nodes.contains(node_index))
            .map(|(_, node)| node.dependency_level)
            .max()
            .unwrap_or_default();
        let terminal_level = (!terminal_nodes.is_empty()).then_some(max_internal_level + 1);
        let mut layers = vec![Vec::new(); terminal_level.unwrap_or(max_internal_level) + 1];
        for (node_index, node) in self.nodes.iter().enumerate() {
            let display_level = if terminal_nodes.contains(&node_index) {
                terminal_level.expect("terminal level exists when terminal nodes exist")
            } else {
                node.dependency_level
            };
            layers[display_level].push(node_index);
        }
        for layer in &mut layers {
            layer.sort_unstable_by(|left, right| {
                self.nodes[*left].name.cmp(&self.nodes[*right].name)
            });
        }

        let mut positions = vec![0; self.nodes.len()];
        update_positions(&layers, &mut positions);
        for _ in 0..3 {
            for layer in layers.iter_mut().skip(1) {
                sort_layer_by_neighbors(
                    layer,
                    &positions,
                    &self.edges,
                    NeighborDirection::Incoming,
                    &self.nodes,
                );
                update_layer_positions(layer, &mut positions);
            }
            let non_final_layers = layers.len().saturating_sub(1);
            for layer in layers.iter_mut().take(non_final_layers).rev() {
                sort_layer_by_neighbors(
                    layer,
                    &positions,
                    &self.edges,
                    NeighborDirection::Outgoing,
                    &self.nodes,
                );
                update_layer_positions(layer, &mut positions);
            }
        }
        layers
    }

    pub fn path_edges_through(&self, node_index: usize) -> HashSet<usize> {
        let mut path_edges = HashSet::new();
        let mut visited = HashSet::from([node_index]);
        let mut queue = VecDeque::from([node_index]);

        while let Some(target) = queue.pop_front() {
            for (edge_index, &(source, edge_target)) in self.edges.iter().enumerate() {
                if edge_target == target {
                    path_edges.insert(edge_index);
                    if visited.insert(source) {
                        queue.push_back(source);
                    }
                }
            }
        }

        visited.clear();
        visited.insert(node_index);
        queue.push_back(node_index);
        while let Some(source) = queue.pop_front() {
            for (edge_index, &(edge_source, target)) in self.edges.iter().enumerate() {
                if edge_source == source {
                    path_edges.insert(edge_index);
                    if visited.insert(target) {
                        queue.push_back(target);
                    }
                }
            }
        }

        path_edges
    }
}

#[derive(Clone, Copy)]
enum NeighborDirection {
    Incoming,
    Outgoing,
}

fn sort_layer_by_neighbors(
    layer: &mut [usize],
    positions: &[usize],
    edges: &[(usize, usize)],
    direction: NeighborDirection,
    nodes: &[DependencyGraphNode],
) {
    layer.sort_by(|left, right| {
        let left_rank = neighbor_rank(*left, positions, edges, direction);
        let right_rank = neighbor_rank(*right, positions, edges, direction);
        compare_ranks(left_rank, right_rank)
            .then_with(|| nodes[*left].name.cmp(&nodes[*right].name))
    });
}

fn neighbor_rank(
    node: usize,
    positions: &[usize],
    edges: &[(usize, usize)],
    direction: NeighborDirection,
) -> Option<(usize, usize)> {
    let neighbors = edges
        .iter()
        .filter_map(|&(source, target)| match direction {
            NeighborDirection::Incoming if target == node => Some(source),
            NeighborDirection::Outgoing if source == node => Some(target),
            _ => None,
        });
    let mut sum = 0;
    let mut count = 0;
    for neighbor in neighbors {
        sum += positions[neighbor];
        count += 1;
    }
    (count > 0).then_some((sum, count))
}

fn compare_ranks(
    left: Option<(usize, usize)>,
    right: Option<(usize, usize)>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some((left_sum, left_count)), Some((right_sum, right_count))) => {
            (left_sum * right_count).cmp(&(right_sum * left_count))
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn update_positions(layers: &[Vec<usize>], positions: &mut [usize]) {
    for layer in layers {
        update_layer_positions(layer, positions);
    }
}

fn update_layer_positions(layer: &[usize], positions: &mut [usize]) {
    for (position, &node) in layer.iter().enumerate() {
        positions[node] = position;
    }
}

fn assign_dependency_levels(
    nodes: &mut [DependencyGraphNode],
    edges: &[(usize, usize)],
) -> (Vec<usize>, Vec<Vec<usize>>) {
    let mut outgoing = vec![Vec::new(); nodes.len()];
    let mut incoming = vec![Vec::new(); nodes.len()];
    for &(source, target) in edges {
        outgoing[source].push(target);
        incoming[target].push(source);
    }

    let mut visited = vec![false; nodes.len()];
    let mut finish_order = Vec::with_capacity(nodes.len());
    for node in 0..nodes.len() {
        visit_for_finish_order(node, &outgoing, &mut visited, &mut finish_order);
    }

    let mut component_by_node = vec![usize::MAX; nodes.len()];
    let mut component_count = 0;
    for &node in finish_order.iter().rev() {
        if component_by_node[node] == usize::MAX {
            assign_component(node, component_count, &incoming, &mut component_by_node);
            component_count += 1;
        }
    }

    let mut component_edges = vec![Vec::new(); component_count];
    let mut component_indegree = vec![0; component_count];
    let mut known_component_edges = HashSet::new();
    for &(source, target) in edges {
        let source_component = component_by_node[source];
        let target_component = component_by_node[target];
        if source_component != target_component
            && known_component_edges.insert((source_component, target_component))
        {
            component_edges[source_component].push(target_component);
            component_indegree[target_component] += 1;
        }
    }
    for targets in &mut component_edges {
        targets.sort_unstable();
    }

    let mut queue: VecDeque<usize> = component_indegree
        .iter()
        .enumerate()
        .filter_map(|(component, &indegree)| (indegree == 0).then_some(component))
        .collect();
    let mut component_levels = vec![0; component_count];
    while let Some(component) = queue.pop_front() {
        for &target in &component_edges[component] {
            component_levels[target] =
                component_levels[target].max(component_levels[component] + 1);
            component_indegree[target] -= 1;
            if component_indegree[target] == 0 {
                queue.push_back(target);
            }
        }
    }

    for (node_index, node) in nodes.iter_mut().enumerate() {
        node.dependency_level = component_levels[component_by_node[node_index]];
    }

    let mut components = vec![Vec::new(); component_count];
    for (node_index, &component) in component_by_node.iter().enumerate() {
        components[component].push(node_index);
    }
    for members in &mut components {
        members.sort_by(|left, right| {
            nodes[*right]
                .is_explicit
                .cmp(&nodes[*left].is_explicit)
                .then_with(|| nodes[*left].name.cmp(&nodes[*right].name))
        });
    }

    (component_by_node, components)
}

fn visit_for_finish_order(
    node: usize,
    outgoing: &[Vec<usize>],
    visited: &mut [bool],
    finish_order: &mut Vec<usize>,
) {
    if std::mem::replace(&mut visited[node], true) {
        return;
    }
    for &target in &outgoing[node] {
        visit_for_finish_order(target, outgoing, visited, finish_order);
    }
    finish_order.push(node);
}

fn assign_component(
    node: usize,
    component: usize,
    incoming: &[Vec<usize>],
    component_by_node: &mut [usize],
) {
    if component_by_node[node] != usize::MAX {
        return;
    }
    component_by_node[node] = component;
    for &source in &incoming[node] {
        assign_component(source, component, incoming, component_by_node);
    }
}

pub fn build_reverse_dependency_graph(
    apps: &[AppItem],
    root_index: usize,
    max_nodes: usize,
) -> DependencyGraph {
    build_dependency_graph(apps, &HashMap::new(), root_index, max_nodes, |app| {
        app.required_by.iter().cloned().collect()
    })
}

pub fn build_forward_dependency_graph(
    apps: &[AppItem],
    provides_map: &HashMap<String, String>,
    root_index: usize,
    max_nodes: usize,
) -> DependencyGraph {
    let mut graph = build_dependency_graph(apps, provides_map, root_index, max_nodes, |app| {
        app.depends_on.clone()
    });
    add_provided_tools(&mut graph, apps, root_index);
    graph
}

fn add_provided_tools(graph: &mut DependencyGraph, apps: &[AppItem], root_index: usize) {
    if apps[root_index].binaries.is_empty() || apps[root_index].is_one_to_one_standalone_tool() {
        return;
    }

    let mut binary_indices: Vec<usize> = (0..apps[root_index].binaries.len()).collect();
    binary_indices.sort_unstable_by(|left, right| {
        apps[root_index].binaries[*left]
            .name
            .cmp(&apps[root_index].binaries[*right].name)
    });
    for binary_index in binary_indices {
        let node_index = graph.nodes.len();
        graph.nodes.push(DependencyGraphNode {
            name: apps[root_index].binaries[binary_index].name.clone(),
            package_index: None,
            kind: DependencyGraphNodeKind::ProvidedTool {
                owner_package_index: root_index,
                binary_index,
            },
            dependency_level: 0,
            is_explicit: false,
            has_hidden_successors: false,
        });
        graph.edges.push((0, node_index));
    }

    let (component_by_node, components) = assign_dependency_levels(&mut graph.nodes, &graph.edges);
    for (node_index, node) in graph.nodes.iter_mut().enumerate() {
        if node_index != 0 && node.kind == DependencyGraphNodeKind::Package {
            node.dependency_level += 1;
        }
    }
    graph.component_by_node = component_by_node;
    graph.components = components;
}

fn build_dependency_graph(
    apps: &[AppItem],
    provides_map: &HashMap<String, String>,
    root_index: usize,
    max_nodes: usize,
    successors: impl Fn(&AppItem) -> Vec<String>,
) -> DependencyGraph {
    let root = &apps[root_index];
    let package_indices: HashMap<&str, usize> = apps
        .iter()
        .enumerate()
        .map(|(index, app)| (app.name.as_str(), index))
        .collect();
    let mut node_indices = HashMap::from([(root.name.clone(), 0)]);
    let mut nodes = vec![DependencyGraphNode {
        name: root.name.clone(),
        package_index: Some(root_index),
        kind: DependencyGraphNodeKind::Package,
        dependency_level: 0,
        is_explicit: root.install_role.is_explicit(),
        has_hidden_successors: false,
    }];
    let mut edges = Vec::new();
    let mut known_edges = HashSet::new();
    let mut queue = VecDeque::from([0]);
    let mut truncated = false;
    let max_nodes = max_nodes.max(1);

    while let Some(source_index) = queue.pop_front() {
        let source_package_index = nodes[source_index].package_index;

        let mut successor_names: Vec<String> = source_package_index
            .map(|index| successors(&apps[index]))
            .unwrap_or_default();
        for successor_name in &mut successor_names {
            if let Some(provider) = provides_map.get(successor_name) {
                successor_name.clone_from(provider);
            }
        }
        successor_names.sort_unstable();
        successor_names.dedup();

        for successor_name in successor_names {
            let target_index = if let Some(&existing_index) = node_indices.get(&successor_name) {
                existing_index
            } else {
                if nodes.len() >= max_nodes {
                    truncated = true;
                    nodes[source_index].has_hidden_successors = true;
                    continue;
                }

                let package_index = package_indices.get(successor_name.as_str()).copied();
                let is_explicit =
                    package_index.is_some_and(|index| apps[index].install_role.is_explicit());
                let target_index = nodes.len();
                node_indices.insert(successor_name.clone(), target_index);
                nodes.push(DependencyGraphNode {
                    name: successor_name,
                    package_index,
                    kind: DependencyGraphNodeKind::Package,
                    dependency_level: 0,
                    is_explicit,
                    has_hidden_successors: false,
                });
                queue.push_back(target_index);
                target_index
            };

            if known_edges.insert((source_index, target_index)) {
                edges.push((source_index, target_index));
            }
        }
    }

    let (component_by_node, components) = assign_dependency_levels(&mut nodes, &edges);

    DependencyGraph {
        nodes,
        edges,
        truncated,
        component_by_node,
        components,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AppItem, BinaryInfo, InstallOrigin, InstallRole, PackageCapabilities, ProgramState,
    };

    fn package(name: &str, role: InstallRole, required_by: &[&str]) -> AppItem {
        AppItem {
            name: name.to_string(),
            version: String::new(),
            origin: InstallOrigin::Pacman,
            install_role: role,
            state: ProgramState::default(),
            size: String::new(),
            install_date: String::new(),
            desc: String::new(),
            url: String::new(),
            licenses: String::new(),
            _owning_pkg: name.to_string(),
            binaries: Vec::new(),
            required_by: required_by
                .iter()
                .map(|package| (*package).to_string())
                .collect(),
            depends_on: Vec::new(),
            desktop_entries: Vec::new(),
            services: Vec::new(),
            capabilities: PackageCapabilities::default(),
        }
    }

    fn package_with_dependencies(name: &str, role: InstallRole, depends_on: &[&str]) -> AppItem {
        let mut app = package(name, role, &[]);
        app.depends_on = depends_on
            .iter()
            .map(|dependency| (*dependency).to_string())
            .collect();
        app
    }

    fn add_tools(app: &mut AppItem, tools: &[&str]) {
        app.binaries = tools
            .iter()
            .map(|tool| BinaryInfo {
                name: (*tool).to_string(),
                dir: "/usr/bin".to_string(),
                path: format!("/usr/bin/{tool}"),
                is_symlink: false,
                target: String::new(),
                version: String::new(),
                _is_pacman_owned: true,
                _owning_pkg: app.name.clone(),
            })
            .collect();
    }

    #[test]
    fn merges_shared_users_and_continues_through_explicit_packages() {
        let apps = vec![
            package("library", InstallRole::Dependency, &["left", "right"]),
            package("left", InstallRole::Dependency, &["application"]),
            package("right", InstallRole::Dependency, &["application"]),
            package("application", InstallRole::Explicit, &["meta-package"]),
            package("meta-package", InstallRole::Explicit, &[]),
        ];

        let graph = build_reverse_dependency_graph(&apps, 0, 20);
        let names: Vec<&str> = graph.nodes.iter().map(|node| node.name.as_str()).collect();

        assert_eq!(
            names,
            ["library", "left", "right", "application", "meta-package"]
        );
        assert_eq!(graph.nodes[3].dependency_level, 2);
        assert_eq!(graph.edges, [(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)]);
        assert!(!graph.is_terminal_component(graph.component_by_node[3]));
        assert!(graph.is_terminal_component(graph.component_by_node[4]));
        assert!(!graph.truncated);
    }

    #[test]
    fn retains_cycle_edges_without_repeating_nodes() {
        let apps = vec![
            package("first", InstallRole::Dependency, &["second"]),
            package("second", InstallRole::Dependency, &["first"]),
        ];

        let graph = build_reverse_dependency_graph(&apps, 0, 20);

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges, [(0, 1), (1, 0)]);
        assert_eq!(
            graph.nodes[0].dependency_level,
            graph.nodes[1].dependency_level
        );
        assert_eq!(graph.component_by_node[0], graph.component_by_node[1]);
        assert!(graph.is_cycle_component(graph.component_by_node[0]));
    }

    #[test]
    fn aligns_only_the_explicit_member_of_a_terminal_cycle_as_an_install_root() {
        let apps = vec![
            package("ffmpeg", InstallRole::Dependency, &["firefox-pure"]),
            package(
                "firefox-pure",
                InstallRole::Explicit,
                &["cachyos-firefox-settings"],
            ),
            package(
                "cachyos-firefox-settings",
                InstallRole::Dependency,
                &["firefox-pure"],
            ),
        ];

        let graph = build_reverse_dependency_graph(&apps, 0, 20);
        let firefox = graph
            .nodes
            .iter()
            .position(|node| node.name == "firefox-pure")
            .expect("firefox package is present");
        let settings = graph
            .nodes
            .iter()
            .position(|node| node.name == "cachyos-firefox-settings")
            .expect("settings package is present");
        let component = graph.component_by_node[firefox];
        let layers = graph.ordered_layers();

        assert_eq!(component, graph.component_by_node[settings]);
        assert!(graph.is_cycle_component(component));
        assert!(graph.is_terminal_component(component));
        assert!(graph.component_has_explicit_package(component));
        assert!(graph.is_aligned_terminal(firefox));
        assert!(!graph.is_aligned_terminal(settings));
        assert_eq!(graph.components[component], [firefox, settings]);
        assert_eq!(
            layers.last().expect("graph has a terminal layer"),
            &[firefox]
        );
        assert!(layers[1].contains(&settings));
        assert_eq!(
            layers.iter().map(Vec::len).sum::<usize>(),
            graph.nodes.len()
        );
    }

    #[test]
    fn caps_broad_graphs() {
        let apps = vec![
            package("library", InstallRole::Dependency, &["one", "two"]),
            package("one", InstallRole::Explicit, &[]),
            package("two", InstallRole::Explicit, &[]),
        ];

        let graph = build_reverse_dependency_graph(&apps, 0, 2);

        assert_eq!(graph.nodes.len(), 2);
        assert!(!graph.is_terminal_component(graph.component_by_node[0]));
        assert!(graph.nodes[0].has_hidden_successors);
        assert!(graph.truncated);
    }

    #[test]
    fn forward_graph_merges_shared_dependencies_and_resolves_providers() {
        let apps = vec![
            package_with_dependencies(
                "application",
                InstallRole::Explicit,
                &["left", "right", "virtual-codec"],
            ),
            package_with_dependencies("left", InstallRole::Dependency, &["shared"]),
            package_with_dependencies("right", InstallRole::Dependency, &["shared"]),
            package_with_dependencies("shared", InstallRole::Dependency, &[]),
            package_with_dependencies("codec-provider", InstallRole::Dependency, &[]),
        ];
        let provides = HashMap::from([("virtual-codec".to_string(), "codec-provider".to_string())]);

        let graph = build_forward_dependency_graph(&apps, &provides, 0, 20);
        let index_by_name: HashMap<&str, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.name.as_str(), index))
            .collect();
        let layers = graph.ordered_layers();
        let terminal_names: HashSet<&str> = layers
            .last()
            .expect("graph has a terminal layer")
            .iter()
            .map(|&node| graph.nodes[node].name.as_str())
            .collect();

        assert_eq!(graph.nodes.len(), 5);
        assert!(!index_by_name.contains_key("virtual-codec"));
        assert!(index_by_name.contains_key("codec-provider"));
        assert!(graph
            .edges
            .contains(&(index_by_name["left"], index_by_name["shared"])));
        assert!(graph
            .edges
            .contains(&(index_by_name["right"], index_by_name["shared"])));
        assert_eq!(terminal_names, HashSet::from(["shared", "codec-provider"]));
    }

    #[test]
    fn forward_graph_places_provided_tools_before_direct_dependencies() {
        let mut suite =
            package_with_dependencies("suite", InstallRole::Explicit, &["direct-dependency"]);
        add_tools(&mut suite, &["tool-z", "tool-a"]);
        let apps = vec![
            suite,
            package_with_dependencies(
                "direct-dependency",
                InstallRole::Dependency,
                &["transitive-dependency"],
            ),
            package_with_dependencies("transitive-dependency", InstallRole::Dependency, &[]),
        ];

        let graph = build_forward_dependency_graph(&apps, &HashMap::new(), 0, 20);
        let layers = graph.ordered_layers();
        let names_in_layer = |level: usize| {
            layers[level]
                .iter()
                .map(|&node| graph.nodes[node].name.as_str())
                .collect::<HashSet<_>>()
        };

        assert_eq!(names_in_layer(0), HashSet::from(["suite"]));
        assert_eq!(names_in_layer(1), HashSet::from(["tool-a", "tool-z"]));
        assert_eq!(names_in_layer(2), HashSet::from(["direct-dependency"]));
        assert_eq!(names_in_layer(3), HashSet::from(["transitive-dependency"]));
        assert!(layers[1].iter().all(|&node| matches!(
            graph.nodes[node].kind,
            DependencyGraphNodeKind::ProvidedTool { .. }
        )));
        assert!(layers[1]
            .iter()
            .all(|&tool| graph.edges.contains(&(0, tool))));
        assert!(graph.edges.iter().all(|&(source, _)| !matches!(
            graph.nodes[source].kind,
            DependencyGraphNodeKind::ProvidedTool { .. }
        )));
    }

    #[test]
    fn forward_graph_flattens_a_one_to_one_standalone_tool() {
        let mut standalone =
            package_with_dependencies("chunk", InstallRole::Standalone, &["direct-dependency"]);
        add_tools(&mut standalone, &["chunk"]);
        let apps = vec![
            standalone,
            package_with_dependencies("direct-dependency", InstallRole::Dependency, &[]),
        ];

        assert!(apps[0].is_one_to_one_standalone_tool());
        let graph = build_forward_dependency_graph(&apps, &HashMap::new(), 0, 20);
        let layers = graph.ordered_layers();

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].name, "direct-dependency");
        assert_eq!(graph.nodes[1].dependency_level, 1);
        assert_eq!(layers.len(), 2);
        assert!(graph
            .nodes
            .iter()
            .all(|node| node.kind == DependencyGraphNodeKind::Package));
    }

    #[test]
    fn reverse_graph_remains_package_only() {
        let mut selected = package("selected", InstallRole::Dependency, &["user"]);
        add_tools(&mut selected, &["provided-tool"]);
        let apps = vec![selected, package("user", InstallRole::Explicit, &[])];

        let graph = build_reverse_dependency_graph(&apps, 0, 20);

        assert_eq!(
            graph
                .nodes
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["selected", "user"]
        );
        assert!(graph
            .nodes
            .iter()
            .all(|node| node.kind == DependencyGraphNodeKind::Package));
    }

    #[test]
    fn orders_layers_by_connected_branches_instead_of_alphabetically() {
        let apps = vec![
            package("selected", InstallRole::Dependency, &["left", "right"]),
            package("left", InstallRole::Dependency, &["zeta-root"]),
            package("right", InstallRole::Dependency, &["alpha-root"]),
            package("zeta-root", InstallRole::Explicit, &[]),
            package("alpha-root", InstallRole::Explicit, &[]),
        ];
        let graph = build_reverse_dependency_graph(&apps, 0, 20);
        let layers = graph.ordered_layers();
        let final_layer: Vec<&str> = layers[2]
            .iter()
            .map(|&node| graph.nodes[node].name.as_str())
            .collect();

        assert_eq!(final_layer, ["zeta-root", "alpha-root"]);
    }

    #[test]
    fn traces_only_paths_that_pass_through_the_hovered_node() {
        let apps = vec![
            package("library", InstallRole::Dependency, &["left", "right"]),
            package("left", InstallRole::Dependency, &["application"]),
            package("right", InstallRole::Dependency, &["application"]),
            package("application", InstallRole::Explicit, &[]),
        ];
        let graph = build_reverse_dependency_graph(&apps, 0, 20);

        assert_eq!(graph.path_edges_through(1), HashSet::from([0, 2]));
    }

    #[test]
    fn ranks_consumers_after_all_packages_they_require() {
        let apps = vec![
            package(
                "selected",
                InstallRole::Dependency,
                &["foundation", "plasma-meta"],
            ),
            package("foundation", InstallRole::Dependency, &["workspace"]),
            package("workspace", InstallRole::Dependency, &["plasma-meta"]),
            package("plasma-meta", InstallRole::Explicit, &[]),
        ];
        let graph = build_reverse_dependency_graph(&apps, 0, 20);
        let level_by_name: HashMap<&str, usize> = graph
            .nodes
            .iter()
            .map(|node| (node.name.as_str(), node.dependency_level))
            .collect();

        assert_eq!(level_by_name["foundation"], 1);
        assert_eq!(level_by_name["workspace"], 2);
        assert_eq!(level_by_name["plasma-meta"], 3);
        assert!(graph.edges.iter().all(|&(source, target)| {
            graph.nodes[source].dependency_level < graph.nodes[target].dependency_level
        }));
    }

    #[test]
    fn aligns_shallow_and_deep_endpoints_in_one_final_layer() {
        let apps = vec![
            package(
                "selected",
                InstallRole::Dependency,
                &["direct-root", "intermediate"],
            ),
            package("direct-root", InstallRole::Explicit, &[]),
            package("intermediate", InstallRole::Dependency, &["deep-root"]),
            package("deep-root", InstallRole::Explicit, &[]),
        ];
        let graph = build_reverse_dependency_graph(&apps, 0, 20);
        let layers = graph.ordered_layers();
        let terminal_names: HashSet<&str> = layers
            .last()
            .expect("graph has a terminal layer")
            .iter()
            .map(|&node| graph.nodes[node].name.as_str())
            .collect();

        assert_eq!(layers.len(), 3);
        assert_eq!(terminal_names, HashSet::from(["direct-root", "deep-root"]));
        assert!(layers
            .last()
            .expect("graph has a terminal layer")
            .iter()
            .all(|&node| graph.is_terminal_component(graph.component_by_node[node])));
    }
}
