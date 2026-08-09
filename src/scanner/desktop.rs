use super::records::{
    binary_for_path, classify_path, resolve_exec_program, upsert, StandaloneRecord,
};
use crate::models::{AppItem, BinaryInfo, DesktopEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) fn scan_and_attach(
    apps: &mut HashMap<String, AppItem>,
    file_owners: &HashMap<String, String>,
) {
    let desktop_entries = scan_entries();

    for entry in desktop_entries {
        let attached_by_owner = file_owners
            .get(&entry.file_path)
            .and_then(|owner| apps.get_mut(owner))
            .map(|app| app.desktop_entries.push(entry.clone()))
            .is_some();
        if attached_by_owner {
            continue;
        }

        let matches: Vec<String> = apps
            .iter()
            .filter(|(_, app)| entry_matches_app(&entry, app))
            .map(|(key, _)| key.clone())
            .collect();
        if !matches.is_empty() {
            for key in matches {
                if let Some(app) = apps.get_mut(&key) {
                    push_entry_once(&mut app.desktop_entries, entry.clone());
                }
            }
            continue;
        }

        if !entry.is_visible {
            continue;
        }
        let Some(program) = resolve_exec_program(&entry.exec, false) else {
            continue;
        };
        let broken = !program.exists() && !program.is_symlink();
        let (origin, state) = classify_path(&program, broken);
        let name = if broken {
            Path::new(&entry.file_path)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(&entry.name)
                .rsplit_once('.')
                .map_or_else(
                    || {
                        Path::new(&entry.file_path)
                            .file_stem()
                            .and_then(|name| name.to_str())
                            .unwrap_or(&entry.name)
                    },
                    |(_, suffix)| suffix,
                )
                .to_string()
        } else {
            program
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(&entry.name)
                .to_string()
        };
        let key = upsert(
            apps,
            StandaloneRecord {
                name,
                version: String::new(),
                origin,
                state,
                description: if broken {
                    format!("Broken desktop launcher: {}", entry.comment)
                } else if entry.comment.is_empty() {
                    format!("Application discovered from {}", entry.file_path)
                } else {
                    entry.comment.clone()
                },
                binary: Some(binary_for_path(&program, "")),
            },
        );
        if let Some(app) = apps.get_mut(&key) {
            push_entry_once(&mut app.desktop_entries, entry);
        }
    }
}

fn push_entry_once(entries: &mut Vec<DesktopEntry>, entry: DesktopEntry) {
    if !entries
        .iter()
        .any(|existing| existing.file_path == entry.file_path)
    {
        entries.push(entry);
    }
}

fn entry_matches_app(entry: &DesktopEntry, app: &AppItem) -> bool {
    let Some(command) = desktop_exec_command(&entry.exec) else {
        return false;
    };
    let command_name = Path::new(&command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&command);

    app.binaries
        .iter()
        .any(|binary| command_matches_binary(&command, command_name, binary))
        || desktop_id_matches_app(entry, &app.name)
}

fn command_matches_binary(command: &str, command_name: &str, binary: &BinaryInfo) -> bool {
    command == binary.path
        || (binary.is_symlink && command == binary.target)
        || (!command.contains('/')
            && (command_name == binary.name
                || (binary.is_symlink
                    && Path::new(&binary.target)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|target_name| command_name == target_name))))
}

fn desktop_id_matches_app(entry: &DesktopEntry, app_name: &str) -> bool {
    Path::new(&entry.file_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|desktop_id| {
            desktop_id.eq_ignore_ascii_case(app_name)
                || desktop_id
                    .rsplit_once('.')
                    .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case(app_name))
        })
}

pub(super) fn desktop_exec_command(exec: &str) -> Option<String> {
    let mut tokens = exec_tokens(exec).into_iter();
    let first = tokens.next()?;
    if first != "env" && !first.ends_with("/env") {
        return (!first.starts_with('%')).then_some(first);
    }

    while let Some(token) = tokens.next() {
        if is_environment_assignment(&token) || token.starts_with('%') {
            continue;
        }
        if matches!(token.as_str(), "-u" | "--unset" | "-C" | "--chdir") {
            tokens.next();
            continue;
        }
        if token == "--" {
            return tokens.next();
        }
        if token.starts_with('-') {
            continue;
        }
        return Some(token);
    }
    None
}

pub(super) fn exec_tokens(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;

    for character in exec.chars() {
        if escaped {
            token.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if quote.is_some_and(|delimiter| character == delimiter) {
            quote = None;
            continue;
        }
        if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        if quote.is_none() && character.is_ascii_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            continue;
        }
        token.push(character);
    }
    if escaped {
        token.push('\\');
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn is_environment_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
            && name
                .chars()
                .next()
                .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
    })
}

fn scan_entries() -> Vec<DesktopEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".to_string());
    let directories = [
        (PathBuf::from(&home).join(".local/share/applications"), 0),
        (PathBuf::from("/usr/share/applications"), 0),
        (PathBuf::from("/usr/local/share/applications"), 0),
        (PathBuf::from("/opt"), 4),
    ];
    let mut desktop_entries = Vec::new();

    for (directory, max_depth) in directories {
        scan_entry_directory(&directory, max_depth, &mut desktop_entries);
    }

    desktop_entries
}

fn scan_entry_directory(directory: &Path, max_depth: usize, entries: &mut Vec<DesktopEntry>) {
    let mut pending = vec![(directory.to_path_buf(), 0_usize)];
    let mut visited = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        let Ok(children) = std::fs::read_dir(directory) else {
            continue;
        };
        for child in children.flatten() {
            visited += 1;
            if visited > 2_000 {
                return;
            }
            let path = child.path();
            if path.is_dir() && !path.is_symlink() && depth < max_depth {
                pending.push((path, depth + 1));
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(entry) = parse_entry(&path, &content) {
                entries.push(entry);
            }
        }
    }
}

fn parse_entry(path: &Path, content: &str) -> Option<DesktopEntry> {
    let mut name = String::new();
    let mut exec = String::new();
    let mut icon = String::new();
    let mut comment = String::new();
    let mut entry_type = String::new();
    let mut no_display = false;
    let mut hidden = false;
    let mut runs_in_terminal = false;
    let mut in_desktop_entry = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }

        if name.is_empty() {
            if let Some(value) = line.strip_prefix("Name=") {
                name = value.trim().to_string();
                continue;
            }
        }
        if exec.is_empty() {
            if let Some(value) = line.strip_prefix("Exec=") {
                exec = value.trim().to_string();
                continue;
            }
        }
        if icon.is_empty() {
            if let Some(value) = line.strip_prefix("Icon=") {
                icon = value.trim().to_string();
                continue;
            }
        }
        if comment.is_empty() {
            if let Some(value) = line.strip_prefix("Comment=") {
                comment = value.trim().to_string();
                continue;
            }
        }
        if entry_type.is_empty() {
            if let Some(value) = line.strip_prefix("Type=") {
                entry_type = value.trim().to_string();
                continue;
            }
        }
        if let Some(value) = line.strip_prefix("NoDisplay=") {
            no_display = value.trim().eq_ignore_ascii_case("true");
            continue;
        }
        if let Some(value) = line.strip_prefix("Hidden=") {
            hidden = value.trim().eq_ignore_ascii_case("true");
            continue;
        }
        if let Some(value) = line.strip_prefix("Terminal=") {
            runs_in_terminal = value.trim().eq_ignore_ascii_case("true");
        }
    }

    if exec.is_empty() {
        return None;
    }
    if name.is_empty() {
        name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
    }

    Some(DesktopEntry {
        file_path: path.to_string_lossy().to_string(),
        name,
        exec,
        icon,
        comment,
        is_visible: (entry_type.is_empty() || entry_type == "Application")
            && !no_display
            && !hidden,
        runs_in_terminal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_the_primary_desktop_entry_section() {
        let content = "[Desktop Entry]\nName=Program Manager\nExec=progs --show\nIcon=progs\nComment=Inspect installed programs\n\n[Desktop Action Other]\nName=Other\nExec=other\n";
        let entry = parse_entry(Path::new("/tmp/progs.desktop"), content).unwrap();

        assert_eq!(entry.name, "Program Manager");
        assert_eq!(entry.exec, "progs --show");
        assert_eq!(entry.icon, "progs");
        assert_eq!(entry.comment, "Inspect installed programs");
        assert_eq!(entry.file_path, "/tmp/progs.desktop");
        assert!(entry.is_visible);
        assert!(!entry.runs_in_terminal);
    }

    #[test]
    fn ignores_entries_without_an_executable() {
        assert!(parse_entry(
            Path::new("/tmp/invalid.desktop"),
            "[Desktop Entry]\nName=Invalid"
        )
        .is_none());
    }

    #[test]
    fn falls_back_to_the_desktop_filename_for_an_empty_name() {
        let entry = parse_entry(
            Path::new("/tmp/program-manager.desktop"),
            "[Desktop Entry]\nExec=progs",
        )
        .unwrap();
        assert_eq!(entry.name, "program-manager");
    }

    #[test]
    fn marks_hidden_and_non_application_entries_as_not_visible() {
        let hidden = parse_entry(
            Path::new("/tmp/hidden.desktop"),
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true",
        )
        .unwrap();
        let service = parse_entry(
            Path::new("/tmp/service.desktop"),
            "[Desktop Entry]\nType=Service\nName=Service\nExec=service",
        )
        .unwrap();

        assert!(!hidden.is_visible);
        assert!(!service.is_visible);
    }

    #[test]
    fn parses_terminal_launchers_and_extracts_env_wrapped_commands() {
        let entry = parse_entry(
            Path::new("/tmp/tool.desktop"),
            "[Desktop Entry]\nType=Application\nName=Tool\nExec=env MODE=fast /usr/bin/tool %F\nTerminal=true",
        )
        .unwrap();

        assert!(entry.runs_in_terminal);
        assert_eq!(
            desktop_exec_command(&entry.exec),
            Some("/usr/bin/tool".to_string())
        );
        assert_eq!(
            desktop_exec_command("\"/opt/My Tool/tool\" --open %F"),
            Some("/opt/My Tool/tool".to_string())
        );
        assert_eq!(
            desktop_exec_command("env -u OLD_MODE MODE=fast tool"),
            Some("tool".to_string())
        );
    }

    #[test]
    fn executable_and_desktop_id_matching_is_exact() {
        let binary = BinaryInfo {
            name: "tool".to_string(),
            dir: "/usr/bin".to_string(),
            path: "/usr/bin/tool".to_string(),
            is_symlink: false,
            target: String::new(),
            version: String::new(),
            _is_pacman_owned: true,
            _owning_pkg: "tool".to_string(),
        };
        assert!(command_matches_binary("/usr/bin/tool", "tool", &binary));
        assert!(command_matches_binary("tool", "tool", &binary));
        assert!(!command_matches_binary(
            "/usr/local/bin/tool",
            "tool",
            &binary
        ));
        assert!(!command_matches_binary(
            "/usr/bin/toolbox",
            "toolbox",
            &binary
        ));

        let entry = parse_entry(
            Path::new("/tmp/org.example.toolbox.desktop"),
            "[Desktop Entry]\nExec=/usr/bin/toolbox",
        )
        .unwrap();
        assert!(desktop_id_matches_app(&entry, "toolbox"));
        assert!(!desktop_id_matches_app(&entry, "tool"));
    }
}
