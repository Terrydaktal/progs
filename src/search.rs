use crate::models::AppItem;
use nucleo_matcher::{
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

pub struct FuzzySearchRanker {
    matcher: Matcher,
    documents: Vec<SearchDocument>,
    last_query: String,
    ranked_indices: Vec<usize>,
    utf32_buffer: Vec<char>,
}

struct SearchDocument {
    app_index: usize,
    name: String,
    sort_name: String,
    commands: Vec<String>,
    metadata: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SearchRank {
    match_class: u8,
    fuzzy_score: u32,
}

#[derive(Clone, Copy)]
enum SearchField {
    PackageName,
    Command,
    Metadata,
}

#[derive(Clone, Copy)]
enum LexicalQuality {
    Fuzzy,
    Substring,
    Prefix,
    Exact,
}

impl Default for FuzzySearchRanker {
    fn default() -> Self {
        let mut config = Config::DEFAULT;
        config.prefer_prefix = true;
        Self {
            matcher: Matcher::new(config),
            documents: Vec::new(),
            last_query: String::new(),
            ranked_indices: Vec::new(),
            utf32_buffer: Vec::new(),
        }
    }
}

impl FuzzySearchRanker {
    pub fn rebuild(&mut self, apps: &[AppItem]) {
        self.documents = apps
            .iter()
            .enumerate()
            .map(|(app_index, app)| SearchDocument::new(app_index, app))
            .collect();
        self.last_query.clear();
        self.ranked_indices = (0..apps.len()).collect();
    }

    pub fn ranked_indices(&mut self, query: &str) -> &[usize] {
        let query = query.trim();
        if query == self.last_query {
            return &self.ranked_indices;
        }

        self.last_query.clear();
        self.last_query.push_str(query);
        if query.is_empty() {
            self.ranked_indices = self
                .documents
                .iter()
                .map(|document| document.app_index)
                .collect();
            return &self.ranked_indices;
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let folded_query = query.to_lowercase();
        let documents = &self.documents;
        let matcher = &mut self.matcher;
        let utf32_buffer = &mut self.utf32_buffer;
        let mut ranked = documents
            .iter()
            .filter_map(|document| {
                let mut best_rank = score_field(
                    &pattern,
                    matcher,
                    utf32_buffer,
                    &document.name,
                    &folded_query,
                    SearchField::PackageName,
                );

                for command in &document.commands {
                    best_rank = best_rank.max(score_field(
                        &pattern,
                        matcher,
                        utf32_buffer,
                        command,
                        &folded_query,
                        SearchField::Command,
                    ));
                }

                best_rank = best_rank.max(score_field(
                    &pattern,
                    matcher,
                    utf32_buffer,
                    &document.metadata,
                    &folded_query,
                    SearchField::Metadata,
                ));
                best_rank.map(|rank| (document.app_index, rank, &document.sort_name))
            })
            .collect::<Vec<_>>();

        ranked.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| left.2.cmp(right.2))
                .then_with(|| left.0.cmp(&right.0))
        });
        self.ranked_indices = ranked
            .into_iter()
            .map(|(app_index, _, _)| app_index)
            .collect();
        &self.ranked_indices
    }
}

impl SearchDocument {
    fn new(app_index: usize, app: &AppItem) -> Self {
        let mut commands = Vec::new();
        for binary in &app.binaries {
            commands.push(binary.name.clone());
            if binary.target != binary.name {
                commands.push(binary.target.clone());
            }
        }
        for desktop_entry in &app.desktop_entries {
            if !desktop_entry.exec.is_empty() {
                commands.push(desktop_entry.exec.clone());
            }
        }

        let dependency_names = app
            .depends_on
            .iter()
            .chain(&app.required_by)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let desktop_metadata = app
            .desktop_entries
            .iter()
            .flat_map(|entry| [&entry.name, &entry.comment])
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let metadata = format!(
            "{} {} {} {} {} {} {} {} {} {} {}",
            app.version,
            app.origin.label(),
            app.install_role.label(),
            app.state.tag_summary(),
            app.capabilities.tag_summary(),
            app.capabilities.primary_role(),
            app.desc,
            app.url,
            app.licenses,
            dependency_names,
            desktop_metadata,
        );

        Self {
            app_index,
            name: app.name.clone(),
            sort_name: app.name.to_lowercase(),
            commands,
            metadata,
        }
    }
}

fn score_field(
    pattern: &Pattern,
    matcher: &mut Matcher,
    utf32_buffer: &mut Vec<char>,
    text: &str,
    folded_query: &str,
    field: SearchField,
) -> Option<SearchRank> {
    if text.is_empty() {
        return None;
    }
    utf32_buffer.clear();
    let fuzzy_score = pattern.score(Utf32Str::new(text, utf32_buffer), matcher)?;
    let quality = match field {
        SearchField::Metadata => LexicalQuality::Fuzzy,
        SearchField::PackageName | SearchField::Command => lexical_quality(text, folded_query),
    };
    Some(SearchRank {
        match_class: match_class(field, quality),
        fuzzy_score,
    })
}

fn lexical_quality(text: &str, folded_query: &str) -> LexicalQuality {
    let folded_text = text.to_lowercase();
    if folded_text == folded_query {
        LexicalQuality::Exact
    } else if folded_text.starts_with(folded_query) {
        LexicalQuality::Prefix
    } else if folded_text.contains(folded_query) {
        LexicalQuality::Substring
    } else {
        LexicalQuality::Fuzzy
    }
}

fn match_class(field: SearchField, quality: LexicalQuality) -> u8 {
    match (field, quality) {
        (SearchField::PackageName, LexicalQuality::Exact) => 9,
        (SearchField::PackageName, LexicalQuality::Prefix) => 8,
        (SearchField::Command, LexicalQuality::Exact) => 7,
        (SearchField::PackageName, LexicalQuality::Substring) => 6,
        (SearchField::Command, LexicalQuality::Prefix) => 5,
        (SearchField::PackageName, LexicalQuality::Fuzzy) => 4,
        (SearchField::Command, LexicalQuality::Substring) => 3,
        (SearchField::Command, LexicalQuality::Fuzzy) => 2,
        (SearchField::Metadata, _) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        BinaryInfo, InstallOrigin, InstallRole, PackageCapabilities, ProgramState,
    };
    use std::collections::HashSet;

    fn app(name: &str, description: &str, commands: &[&str]) -> AppItem {
        AppItem {
            name: name.to_string(),
            version: "1.0".to_string(),
            origin: InstallOrigin::Pacman,
            install_role: InstallRole::Explicit,
            state: ProgramState::default(),
            size: String::new(),
            install_date: String::new(),
            desc: description.to_string(),
            url: String::new(),
            licenses: String::new(),
            _owning_pkg: name.to_string(),
            binaries: commands
                .iter()
                .map(|command| BinaryInfo {
                    name: (*command).to_string(),
                    dir: "/usr/bin".to_string(),
                    path: format!("/usr/bin/{command}"),
                    is_symlink: false,
                    target: format!("/usr/bin/{command}"),
                    version: "1.0".to_string(),
                    _is_pacman_owned: true,
                    _owning_pkg: name.to_string(),
                })
                .collect(),
            required_by: HashSet::new(),
            depends_on: Vec::new(),
            desktop_entries: Vec::new(),
            services: Vec::new(),
            capabilities: PackageCapabilities::default(),
        }
    }

    #[test]
    fn tolerates_omitted_characters_and_ranks_package_names() {
        let apps = vec![
            app("plasma-meta", "desktop environment", &[]),
            app("spectacle", "screenshot utility", &[]),
            app("unrelated", "spectcle metadata", &[]),
        ];
        let mut ranker = FuzzySearchRanker::default();
        ranker.rebuild(&apps);

        assert_eq!(ranker.ranked_indices("spectcle"), &[1, 2]);
    }

    #[test]
    fn exact_command_matches_rank_above_metadata_matches() {
        let apps = vec![
            app("aircrack-ng", "wireless tools", &["buddy-ng"]),
            app("documentation", "buddy-ng command reference", &[]),
        ];
        let mut ranker = FuzzySearchRanker::default();
        ranker.rebuild(&apps);

        assert_eq!(ranker.ranked_indices("buddy-ng"), &[0, 1]);
    }

    #[test]
    fn exact_package_match_wins_and_empty_query_keeps_scan_order() {
        let apps = vec![
            app("ffmpeg-tools", "media", &[]),
            app("ffmpeg", "media", &[]),
        ];
        let mut ranker = FuzzySearchRanker::default();
        ranker.rebuild(&apps);

        assert_eq!(ranker.ranked_indices(""), &[0, 1]);
        assert_eq!(ranker.ranked_indices("ffmpeg"), &[1, 0]);
    }
}
