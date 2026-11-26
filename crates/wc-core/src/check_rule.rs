//! Check rules the user writes for a project: house rules that run with the built-in project
//! check and show up in the same list (for example "every motor has a cable", "terminals are
//! tagged `-X<n>.<n>`" or "at most two wires on a terminal").
//!
//! They are kept in `project.toml` as `[[rule]]` tables:
//!
//! ```toml
//! # Every motor has a cable on one of its pins.
//! [[rule]]
//! kind = "cable"
//! devices = "-M*"
//!
//! # Terminals are tagged like -X1.3.
//! [[rule]]
//! kind = "tag"
//! role = "terminal"
//! pattern = "-X#.#"
//! severity = "error"
//!
//! # At most two wires on one terminal pin.
//! [[rule]]
//! kind = "max-wires"
//! max = 2
//! ```
//!
//! Every rule picks the placed symbols it applies to by `role` (`main`, `auxiliary`,
//! `standalone`, `terminal`, `interruption`) and by `devices`, a tag pattern; leaving both out
//! picks every tagged symbol (for `max-wires`: every terminal). Overview pages do not count.
//!
//! Tag patterns use `*` for any run of characters, `?` for one character and `#` for a number
//! (one or more digits). A pattern starting with `-` also matches the product part of a tag with
//! a function or location, so `-M*` matches `=A1+L2-M3`.
//!
//! A library source can carry house rules too: a `rules.toml` in the source's folder with the
//! same `[[rule]]` tables, run for every project that uses the source, before the project's
//! own rules. Their findings name the source (`check rule 2 of library house`). A rule may
//! have a `name`; a project turns a library rule off by number or name in `project.toml`:
//!
//! ```toml
//! rules_off = ["house:2", "house:motor-cable"]
//! ```
//!
//! `severity` is `error`, `warning` (the default) or `note`. `message` replaces the built-in
//! text; `{tag}` in it is replaced by the device's tag. A rule that cannot run (missing or
//! unknown kind, a key it does not know such as a misspelt `severty`, missing pattern, bad role)
//! is reported by the check itself instead of being skipped quietly; unknown keys are kept when
//! the project is saved.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::DeviceTag;
use crate::symbol::SymbolRole;

/// One user-written check rule.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CheckRule {
    /// A name to turn the rule off by (a library rule, [`ProjectMeta::rules_off`]) and to
    /// show in its findings.
    ///
    /// [`ProjectMeta::rules_off`]: crate::project::ProjectMeta::rules_off
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the rule checks: `cable`, `tag` or `max-wires`. Empty when the table leaves it
    /// out, which the check reports.
    #[serde(default)]
    pub kind: String,
    /// The devices it applies to, by tag pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<String>,
    /// The symbol role it applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// For `tag`: the pattern every picked tag must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// For `max-wires`: the most wire ends allowed on one pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    /// `error`, `warning` or `note`; `None` is a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// A message in place of the built-in one; `{tag}` stands for the device's tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Keys the rule does not know (a misspelt `severty`), kept so a save writes them back,
    /// and reported by the check instead of being ignored.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// The file in a library source's folder that holds its check rules.
pub const LIBRARY_RULES_FILE: &str = "rules.toml";

/// The check rules of one library source: its name, and its rules, or why its
/// [`LIBRARY_RULES_FILE`] could not be read.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleSet {
    pub source: String,
    pub rules: Result<Vec<CheckRule>, String>,
}

/// The `[[rule]]` tables of a rules file.
pub fn parse_rules(text: &str) -> Result<Vec<CheckRule>, String> {
    #[derive(Deserialize)]
    struct File {
        #[serde(default)]
        rule: Vec<CheckRule>,
    }
    toml::from_str::<File>(text)
        .map(|f| f.rule)
        .map_err(|e| e.message().to_owned())
}

/// Whether `off` (a project's [`ProjectMeta::rules_off`]) turns off rule `n` (from 1) of the
/// library source `source`: by `source:n` or `source:name`.
///
/// [`ProjectMeta::rules_off`]: crate::project::ProjectMeta::rules_off
pub fn turned_off(off: &[String], source: &str, n: usize, rule: &CheckRule) -> bool {
    off.iter().any(|o| {
        let Some((s, which)) = o.trim().split_once(':') else {
            return false;
        };
        let which = which.trim();
        s.trim() == source
            && (which.parse::<usize>().ok() == Some(n) || rule.name.as_deref().is_some_and(|nm| nm.trim() == which))
    })
}

/// The kinds of rule there are, as written in `kind`.
pub const KINDS: [&str; 3] = ["cable", "tag", "max-wires"];

impl CheckRule {
    /// The role named in `role`: `Ok(None)` when none is named, `Err` with the text when it is
    /// not a role.
    pub fn role(&self) -> Result<Option<SymbolRole>, String> {
        let Some(r) = &self.role else { return Ok(None) };
        let role = match r.trim().to_ascii_lowercase().as_str() {
            "main" => SymbolRole::Main,
            "auxiliary" => SymbolRole::Auxiliary,
            "standalone" => SymbolRole::Standalone,
            "terminal" => SymbolRole::Terminal,
            "interruption" => SymbolRole::Interruption,
            _ => return Err(r.clone()),
        };
        Ok(Some(role))
    }

    /// Whether a placed symbol with `tag` and symbol `role` is one this rule looks at, given
    /// the rule's own (already checked) role. `default_role` is used when the rule names
    /// neither a role nor devices.
    pub fn picks(
        &self,
        tag: &DeviceTag,
        role: Option<SymbolRole>,
        rule_role: Option<SymbolRole>,
        default_role: Option<SymbolRole>,
    ) -> bool {
        let want = match (rule_role, &self.devices) {
            (Some(r), _) => Some(r),
            (None, None) => default_role,
            (None, Some(_)) => None,
        };
        if want.is_some_and(|w| role != Some(w)) {
            return false;
        }
        self.devices.as_deref().is_none_or(|p| tag_matches(p, tag))
    }

    /// The finding's text: the rule's own `message` with `{tag}` filled in, or `default`.
    pub fn text(&self, tag: &DeviceTag, default: String) -> String {
        match &self.message {
            Some(m) => m.replace("{tag}", &tag.to_string()),
            None => default,
        }
    }
}

/// Whether `tag` matches the tag pattern `pattern` (see the module docs).
pub fn tag_matches(pattern: &str, tag: &DeviceTag) -> bool {
    let pattern = pattern.trim();
    if glob(pattern, &tag.to_string()) {
        return true;
    }
    pattern.starts_with('-')
        && (!tag.function.is_empty() || !tag.location.is_empty())
        && glob(pattern, &format!("-{}", tag.product.join(".")))
}

/// Whether `text` matches `pattern` as a whole: `*` is any run of characters, `?` one
/// character, `#` one or more digits; every other character stands for itself.
pub fn glob(pattern: &str, text: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    // at[k]: the pattern so far can end right before the k-th character. Each pattern character
    // takes one pass over the text, so a match costs pattern length times text length.
    let mut at: Vec<bool> = std::iter::once(true).chain(t.iter().map(|_| false)).collect();
    for c in pattern.chars() {
        let mut next = Vec::with_capacity(at.len());
        next.push(c == '*' && at.first().copied().unwrap_or(false));
        for ((ch, &before), &here) in t.iter().zip(&at).zip(at.iter().skip(1)) {
            let prev = next.last().copied().unwrap_or(false);
            next.push(match c {
                '*' => prev || here,
                '#' => ch.is_ascii_digit() && (before || prev),
                _ => before && (c == '?' || *ch == c),
            });
        }
        if !next.contains(&true) {
            return false;
        }
        at = next;
    }
    at.last().copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::project::ProjectMeta;

    fn tag(s: &str) -> DeviceTag {
        DeviceTag::parse(s).unwrap()
    }

    #[test]
    fn a_rules_file_reads_its_rule_tables_and_says_what_is_wrong() {
        let rules = parse_rules(
            "[[rule]]\nname = \"motor-cable\"\nkind = \"cable\"\ndevices = \"-M*\"\n\n[[rule]]\nkind = \"max-wires\"\nmax = 2\n",
        )
        .unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].name.as_deref(), Some("motor-cable"));
        assert_eq!(rules[1].max, Some(2));
        assert!(parse_rules("").unwrap().is_empty());
        assert!(parse_rules("[[rule]]\nmax = \"two\"").is_err());
    }

    #[test]
    fn a_library_rule_is_turned_off_by_source_and_number_or_name() {
        let rule = CheckRule {
            name: Some("motor-cable".into()),
            ..CheckRule::default()
        };
        let off = |o: &[&str]| o.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(turned_off(&off(&["house:2"]), "house", 2, &rule));
        assert!(turned_off(&off(&[" house : motor-cable "]), "house", 1, &rule));
        assert!(!turned_off(&off(&["house:2"]), "house", 1, &rule));
        assert!(!turned_off(&off(&["other:2"]), "house", 2, &rule));
        assert!(!turned_off(&off(&["house"]), "house", 2, &rule));
        assert!(!turned_off(&off(&["house:cable"]), "house", 2, &CheckRule::default()));
    }

    #[test]
    fn globs_match_stars_marks_and_numbers() {
        assert!(glob("-M*", "-M1"));
        assert!(glob("-M*", "-M"));
        assert!(!glob("-M*", "-K1"));
        assert!(glob("-X#.#", "-X1.23"));
        assert!(!glob("-X#.#", "-X1."));
        assert!(!glob("-X#.#", "-XA.1"));
        assert!(!glob("-X#", "-X"));
        assert!(glob("-K?", "-K1"));
        assert!(!glob("-K?", "-K12"));
        assert!(glob("*", ""));
        assert!(glob("=A*-K#", "=A1+L2-K10"));
        assert!(glob("#*#", "12"));
        assert!(!glob("#*#", "1"));
        assert!(glob("-K#?", "-K12a"));
        assert!(!glob("", "x"));
        assert!(glob("", ""));
        // A long numeric tag against stars and numbers stays one pass per pattern character.
        let long = format!("-M{}", "1".repeat(200_000));
        assert!(glob("-M*#*#", &long));
        assert!(!glob("-M*#*#x", &long));
    }

    #[test]
    fn a_product_pattern_also_matches_a_tag_with_function_and_location() {
        assert!(tag_matches("-M*", &tag("=A1+L2-M3")));
        assert!(tag_matches("-M*", &tag("-M3")));
        assert!(!tag_matches("+L2*", &tag("=A1+L2-M3")));
        assert!(tag_matches("*+L2*", &tag("=A1+L2-M3")));
    }

    #[test]
    fn a_rule_picks_by_role_and_devices() {
        let rule = CheckRule {
            kind: "tag".into(),
            role: Some("Terminal".into()),
            ..CheckRule::default()
        };
        let role = rule.role().unwrap();
        assert!(rule.picks(&tag("-X1.1"), Some(SymbolRole::Terminal), role, None));
        assert!(!rule.picks(&tag("-K1"), Some(SymbolRole::Main), role, None));
        let by_tag = CheckRule {
            kind: "cable".into(),
            devices: Some("-M*".into()),
            ..CheckRule::default()
        };
        assert!(by_tag.picks(&tag("-M1"), None, None, Some(SymbolRole::Terminal)));
        assert!(!by_tag.picks(&tag("-K1"), None, None, None));
        let bad = CheckRule {
            role: Some("coil".into()),
            ..CheckRule::default()
        };
        assert_eq!(bad.role(), Err("coil".into()));
    }

    #[test]
    fn rules_read_and_write_as_rule_tables_in_project_toml() {
        let text = r#"
name = "x"

[[rule]]
kind = "cable"
devices = "-M*"
message = "{tag} needs a motor cable"

[[rule]]
kind = "max-wires"
max = 2
severity = "error"
"#;
        let meta: ProjectMeta = toml::from_str(text).unwrap();
        assert_eq!(meta.rules.len(), 2);
        assert_eq!(
            meta.rules[0].text(&tag("-M1"), String::new()),
            "-M1 needs a motor cable"
        );
        assert_eq!(meta.rules[1].max, Some(2));
        let again: ProjectMeta = toml::from_str(&toml::to_string(&meta).unwrap()).unwrap();
        assert_eq!(again.rules, meta.rules);
        assert!(!toml::to_string(&ProjectMeta::new("y")).unwrap().contains("rule"));
    }

    #[test]
    fn a_rule_without_kind_or_with_unknown_keys_still_loads_and_keeps_them() {
        let text = r#"
name = "x"

[[rule]]
devices = "-M*"

[[rule]]
kind = "tag"
pattern = "-K#"
severty = "error"
"#;
        let meta: ProjectMeta = toml::from_str(text).unwrap();
        assert_eq!(meta.rules[0].kind, "");
        assert_eq!(meta.rules[0].devices.as_deref(), Some("-M*"));
        assert_eq!(meta.rules[1].severity, None);
        assert_eq!(
            meta.rules[1].unknown.keys().collect::<Vec<_>>(),
            ["severty"],
            "a misspelt key is kept"
        );
        let saved = toml::to_string(&meta).unwrap();
        assert!(saved.contains("severty = \"error\""), "{saved}");
        let again: ProjectMeta = toml::from_str(&saved).unwrap();
        assert_eq!(again.rules, meta.rules);
    }
}
