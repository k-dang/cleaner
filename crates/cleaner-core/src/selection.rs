//! Selection format and reconciliation against the app's built-in catalog.
//! The native app owns locating, safely reading, and durably replacing the file.

use std::collections::BTreeMap;
use std::io;

use crate::targets::Target;

pub type Choices = BTreeMap<String, bool>;

pub struct Loaded {
    pub choices: Choices,
    pub needs_save: bool,
}

/// Choices for every built-in Target, selected where `pick` says so.
pub fn choices<'a>(
    targets: impl IntoIterator<Item = &'a Target>,
    pick: impl Fn(&Target) -> bool,
) -> Choices {
    targets
        .into_iter()
        .map(|target| (target.id.to_string(), pick(target)))
        .collect()
}

pub fn defaults<'a>(targets: impl IntoIterator<Item = &'a Target>) -> Choices {
    choices(targets, |target| target.default_selected)
}

/// Keeps explicit choices, adds new Targets at their defaults, and drops obsolete IDs.
/// Malformed data is an error; callers must not treat it as a first run.
pub fn parse<'a>(
    contents: &[u8],
    targets: impl IntoIterator<Item = &'a Target>,
) -> io::Result<Loaded> {
    let value: serde_json::Value = serde_json::from_slice(contents)?;
    let object = value.as_object().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Selection must be a JSON object",
        )
    })?;
    if object.values().any(|value| !value.is_boolean()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Selection values must be booleans",
        ));
    }
    let mut choices = defaults(targets);
    let mut needs_save = object.len() != choices.len();
    for (id, selected) in &mut choices {
        match object.get(id) {
            Some(value) => *selected = value.as_bool().unwrap(),
            None => needs_save = true,
        }
    }
    Ok(Loaded {
        choices,
        needs_save,
    })
}

/// Encodes one complete Selection. Validation happens before the app writes anything.
pub fn serialize<'a>(
    choices: &Choices,
    targets: impl IntoIterator<Item = &'a Target>,
) -> io::Result<Vec<u8>> {
    let expected = defaults(targets);
    if choices.len() != expected.len() || expected.keys().any(|id| !choices.contains_key(id)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "incomplete Selection",
        ));
    }
    let mut contents = serde_json::to_vec(choices)?;
    contents.push(b'\n');
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    static TARGETS: [Target; 2] = [
        Target {
            id: "cache",
            name: "Cache",
            category: "System",
            default_selected: true,
            min_age: None,
        },
        Target {
            id: "build",
            name: "Build cache",
            category: "Developer",
            default_selected: false,
            min_age: None,
        },
    ];

    #[test]
    fn explicit_unticks_survive_serialization_and_restart() {
        let mut selected = defaults(&TARGETS);
        selected.insert("cache".into(), false);
        let loaded = parse(&serialize(&selected, &TARGETS).unwrap(), &TARGETS).unwrap();
        assert_eq!(loaded.choices, selected);
        assert!(!loaded.needs_save);
    }

    #[test]
    fn missing_new_ids_take_defaults_and_obsolete_ids_are_ignored() {
        let loaded = parse(br#"{"cache":false,"obsolete":true}"#, &TARGETS).unwrap();
        assert!(loaded.needs_save);
        assert!(!loaded.choices["cache"]);
        assert!(!loaded.choices["build"]);
        assert!(!loaded.choices.contains_key("obsolete"));
    }

    #[test]
    fn malformed_selections_are_errors_even_for_obsolete_ids() {
        for contents in [
            b"{".as_slice(),
            b"[]",
            b"null",
            br#"{"cache":1}"#,
            br#"{"obsolete":"yes"}"#,
        ] {
            assert!(parse(contents, &TARGETS).is_err(), "accepted {contents:?}");
        }
    }

    #[test]
    fn serialization_rejects_missing_and_unknown_ids() {
        let mut selected = defaults(&TARGETS);
        selected.remove("build");
        assert!(serialize(&selected, &TARGETS).is_err());
        selected.insert("arbitrary-path".into(), true);
        assert!(serialize(&selected, &TARGETS).is_err());
    }
}
