//! Preset files: the plugin's full state (every parameter, hand-placed notes
//! and step pitches) as JSON, one file per preset, in
//! `<home>/Documents/Euclid Seq/Presets`. Loading goes through the host
//! (`GuiContext::set_state`), so older presets are migrated like old projects.

use nih_plug::prelude::PluginState;
use std::path::PathBuf;

const EXTENSION: &str = "json";

/// The preset folder (not created yet).
pub fn dir() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }?;
    Some(PathBuf::from(home).join("Documents").join("Euclid Seq").join("Presets"))
}

/// A name that is safe as a file name on every system: no path characters,
/// trimmed, at most 60 characters.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(60)
        .collect();
    cleaned.trim().trim_matches('.').trim().to_string()
}

fn path_for(name: &str) -> Option<PathBuf> {
    let name = sanitize(name);
    if name.is_empty() {
        return None;
    }
    Some(dir()?.join(format!("{name}.{EXTENSION}")))
}

/// Preset names in the folder, sorted case-insensitively.
pub fn list() -> Vec<String> {
    let Some(dir) = dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION)))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub fn exists(name: &str) -> bool {
    path_for(name).is_some_and(|p| p.exists())
}

/// "Preset 1", "Preset 2", … : the first name not taken yet.
pub fn next_free_name() -> String {
    (1..)
        .map(|n| format!("Preset {n}"))
        .find(|name| !exists(name))
        .unwrap_or_else(|| "Preset".to_string())
}

pub fn save(name: &str, state: &PluginState) -> Result<String, String> {
    let name = sanitize(name);
    let path = path_for(&name).ok_or("Give the preset a name")?;
    let dir = path.parent().ok_or("No preset folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't create the preset folder: {e}"))?;
    let json = serde_json::to_string_pretty(state).map_err(|e| format!("Can't save: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("Can't save: {e}"))?;
    Ok(name)
}

pub fn load(name: &str) -> Result<PluginState, String> {
    let path = path_for(name).ok_or("No such preset")?;
    let json = std::fs::read_to_string(&path).map_err(|e| format!("Can't open '{name}': {e}"))?;
    serde_json::from_str(&json).map_err(|_| format!("'{name}' isn't a Euclid Seq preset"))
}

/// Show the preset folder in Finder / Explorer / the file manager.
pub fn open_folder() -> Result<(), String> {
    let dir = dir().ok_or("No preset folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Can't create the preset folder: {e}"))?;
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Can't open the folder: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nih_plug::wrapper::state::ParamValue;
    use std::collections::BTreeMap;

    #[test]
    fn names_are_safe_file_names() {
        assert_eq!(sanitize("  Dark / Pads: v2?  "), "Dark  Pads v2");
        assert_eq!(sanitize("..\\..\\evil"), "evil");
        assert_eq!(sanitize("   "), "");
        assert_eq!(sanitize(&"x".repeat(100)).len(), 60);
    }

    #[test]
    fn save_list_load_round_trip() {
        // Point the preset folder at a scratch home directory.
        let home = std::env::temp_dir().join(format!("euclid_seq_test_{}", std::process::id()));
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let old = std::env::var_os(var);
        std::env::set_var(var, &home);

        let mut params = BTreeMap::new();
        params.insert("pitch_1".to_string(), ParamValue::I32(7));
        let mut fields = BTreeMap::new();
        fields.insert("added_1".to_string(), "5".to_string());
        let state = PluginState { version: "0.10.0".into(), params, fields };

        assert_eq!(next_free_name(), "Preset 1");
        assert_eq!(save("My Groove", &state).unwrap(), "My Groove");
        assert_eq!(save("Preset 1", &state).unwrap(), "Preset 1");
        assert_eq!(list(), vec!["My Groove", "Preset 1"]);
        assert!(exists("My Groove"));
        assert_eq!(next_free_name(), "Preset 2");

        let loaded = load("My Groove").unwrap();
        assert!(matches!(loaded.params["pitch_1"], ParamValue::I32(7)));
        assert_eq!(loaded.fields["added_1"], "5");
        assert!(load("Missing").is_err());

        std::fs::write(dir().unwrap().join("Broken.json"), "not json").unwrap();
        assert!(load("Broken").unwrap_err().contains("isn't a Euclid Seq preset"));

        let _ = std::fs::remove_dir_all(&home);
        match old {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }
}
