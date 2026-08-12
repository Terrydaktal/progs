use crate::models::AppItem;
use fuzzy_rank::fields::fuzzy::{MetadataQuery, PreparedMetadataCandidate, PreparedMetadataField};
use fuzzy_rank::ranking::SearchRank;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct FuzzySearchRanker {
    documents: Vec<SearchDocument>,
    relationship_index: HashMap<String, Vec<usize>>,
    last_query: String,
    ranked_indices: Vec<usize>,
}

struct SearchDocument {
    app_index: usize,
    sort_name: String,
    fields: Vec<PreparedMetadataField>,
}

impl FuzzySearchRanker {
    pub fn rebuild(&mut self, apps: &[AppItem]) {
        self.documents = apps
            .iter()
            .enumerate()
            .map(|(app_index, app)| SearchDocument::new(app_index, app))
            .collect();
        self.relationship_index.clear();
        for (app_index, app) in apps.iter().enumerate() {
            for relationship in app.depends_on.iter().chain(&app.required_by) {
                let relationship = relationship_key(relationship);
                if !relationship.is_empty() {
                    self.relationship_index
                        .entry(relationship)
                        .or_default()
                        .push(app_index);
                }
            }
        }
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

        let search_text = query.to_string();
        let Some(query) = MetadataQuery::new(query).map(|query| query.with_typo_fallback(true))
        else {
            self.ranked_indices.clear();
            return &self.ranked_indices;
        };

        let mut tiers = std::array::from_fn::<_, 10, _>(|_| Vec::new());
        let folded_query = search_text.to_lowercase();
        for document in &self.documents {
            let candidate = PreparedMetadataCandidate {
                key: &document.sort_name,
                fields: &document.fields,
                score: 0.0,
            };
            if let Some(rank) = query.search_rank_prepared(candidate) {
                let match_class = match_class(&candidate, &folded_query, &rank);
                tiers[match_class as usize].push((candidate, rank, document.app_index));
            }
        }

        // Sort each relevance tier once.  Calling compare_candidates from a
        // comparison closure rebuilt all lazy metadata relevance for every
        // comparison, turning a small result set into a large amount of
        // repeated tokenisation and edit-distance work.
        self.ranked_indices.clear();
        for class in (0..=9).rev() {
            query.sort_matches_prepared_with(&mut tiers[class]);
            self.ranked_indices
                .extend(tiers[class].drain(..).map(|(_, _, app_index)| app_index));
        }

        // Relationships are indexed separately and only match exactly.  They
        // remain useful search targets without making every fuzzy query scan
        // thousands of dependency and reverse-dependency names.
        let relationship_key = relationship_key(&search_text);
        let mut seen: HashSet<usize> = self.ranked_indices.iter().copied().collect();
        if let Some(indices) = self.relationship_index.get(&relationship_key) {
            for &app_index in indices {
                if seen.insert(app_index) {
                    self.ranked_indices.push(app_index);
                }
            }
        }
        &self.ranked_indices
    }
}

// Keep the app-level relevance tiers explicit: a package name or command is
// more useful than a match in descriptive metadata, while fuzzy-rank decides
// the finer ordering inside each tier.
fn match_class(candidate: &PreparedMetadataCandidate<'_>, query: &str, rank: &SearchRank) -> u8 {
    let provenance = rank.provenance();
    let field = candidate
        .fields
        .get(provenance.field_index)
        .map(|field| field.value.as_str())
        .unwrap_or_default();
    let quality = if field == query {
        3
    } else if field.starts_with(query) {
        2
    } else if field.contains(query) {
        1
    } else {
        0
    };

    let priority = provenance.field_priority;
    match priority {
        0 => match quality {
            3 => 9,
            2 => 8,
            1 => 6,
            _ => 4,
        },
        1 => match quality {
            3 => 7,
            2 => 5,
            1 => 3,
            _ => 2,
        },
        _ => 1,
    }
}

impl SearchDocument {
    fn new(app_index: usize, app: &AppItem) -> Self {
        let mut fields = Vec::new();
        push_field(&mut fields, 0, &app.name);

        for binary in &app.binaries {
            push_field(&mut fields, 1, &binary.name);
            if !binary.target.is_empty() && binary.target != binary.name {
                push_field(&mut fields, 1, &binary.target);
            }
        }
        for desktop_entry in &app.desktop_entries {
            push_field(&mut fields, 1, &desktop_entry.exec);
            push_field(&mut fields, 2, &desktop_entry.name);
            push_field(&mut fields, 3, &desktop_entry.comment);
        }

        push_field(&mut fields, 3, &app.desc);
        push_field(&mut fields, 4, &app.version);
        push_field(&mut fields, 4, app.origin.label());
        push_field(&mut fields, 4, app.install_role.label());
        push_field(&mut fields, 4, app.state.tag_summary());
        push_field(&mut fields, 4, app.capabilities.tag_summary());
        push_field(&mut fields, 4, app.capabilities.primary_role());
        push_field(&mut fields, 4, &app.url);
        push_field(&mut fields, 4, &app.licenses);

        Self {
            app_index,
            sort_name: app.name.to_lowercase(),
            fields,
        }
    }
}

fn push_field(fields: &mut Vec<PreparedMetadataField>, priority: u8, value: impl AsRef<str>) {
    let value = value.as_ref().trim();
    if let Some(field) = PreparedMetadataField::new(priority, value) {
        fields.push(field);
    }
}

fn relationship_key(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
            representative_path: String::new(),
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

    #[test]
    fn exact_relationship_matches_use_the_dedicated_index() {
        let mut consumer = app("consumer", "application", &[]);
        consumer.depends_on = vec!["provider-package".to_string()];
        let apps = vec![consumer];
        let mut ranker = FuzzySearchRanker::default();
        ranker.rebuild(&apps);

        assert_eq!(ranker.ranked_indices("provider.package"), &[0]);
    }
}
