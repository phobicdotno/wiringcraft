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

/// The roles a rule can name, as written in `role`.
pub const ROLES: [&str; 5] = ["main", "auxiliary", "standalone", "terminal", "interruption"];
/// The severities a rule can have, as written in `severity`.
pub const SEVERITIES: [&str; 3] = ["error", "warning", "note"];

/// Rule `n` (from 1, for the error) as the check runs it, or why the check would report it
/// instead of running it: a kind it knows (any case, kept lower case), a pattern for a `tag`
/// rule, a maximum for a `max-wires` rule, a known role and severity (any case). Values only
/// another kind uses are dropped and empty values are left out. Keys the rule does not know
/// are kept; [`check_rule`] refuses them.
pub fn checked(rule: &CheckRule, n: usize) -> Result<CheckRule, String> {
    let set = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
    let kind = rule.kind.trim().to_ascii_lowercase();
    let kind = kind.as_str();
    if !KINDS.contains(&kind) {
        return Err(format!("Rule {n}: pick a kind ({}).", KINDS.join(", ")));
    }
    // Only a max-wires rule uses a maximum.
    let max = rule.max.filter(|_| kind == "max-wires");
    if kind == "max-wires" && max.is_none() {
        return Err(format!("Rule {n}: say how many wires a pin may have at most."));
    }
    let pattern = set(&rule.pattern);
    if kind == "tag" && pattern.is_none() {
        return Err(format!("Rule {n}: a tag rule needs the pattern every tag must match."));
    }
    // The check runs no rule with another role or severity (it reports it instead).
    let known = |v: &Option<String>, of: &[&str]| {
        v.as_deref()
            .is_none_or(|v| of.contains(&v.to_ascii_lowercase().as_str()))
    };
    let (role, severity) = (set(&rule.role), set(&rule.severity));
    if !known(&role, &ROLES) {
        return Err(format!("Rule {n}: pick a role ({}), or none.", ROLES.join(", ")));
    }
    if !known(&severity, &SEVERITIES) {
        return Err(format!("Rule {n}: pick a severity ({}).", SEVERITIES.join(", ")));
    }
    Ok(CheckRule {
        name: rule.name.clone(),
        kind: kind.to_owned(),
        devices: set(&rule.devices),
        role,
        // Only a tag rule uses a pattern.
        pattern: if kind == "tag" { pattern } else { None },
        max,
        severity,
        message: set(&rule.message),
        unknown: rule.unknown.clone(),
    })
}

/// A rule written new (from a file or a tool) as [`checked`] keeps it, refusing keys a rule
/// does not know: the check would only report them.
pub fn check_rule(rule: &CheckRule, n: usize) -> Result<CheckRule, String> {
    if !rule.unknown.is_empty() {
        let keys: Vec<&str> = rule.unknown.keys().map(String::as_str).collect();
        return Err(format!(
            "Rule {n}: no such key {} (a rule has kind, devices, role, pattern, max, severity, message and name).",
            keys.join(", ")
        ));
    }
    checked(rule, n)
}

/// The one library rule `which` names (`source:number` or `source:name`, as [`turned_off`]
/// reads it): its source, its number and the rule. `Ok(None)` when it names none (no such
/// source, number or name). An error when the source's rules cannot be read (with why), or
/// when it could stand for two rules (a rule named "2" and rule 2, or two rules of one name),
/// since an entry in `rules_off` would turn off all of them.
pub fn library_rule<'a>(sets: &'a [RuleSet], which: &str) -> Result<Option<(&'a str, usize, &'a CheckRule)>, String> {
    let which = which.trim();
    let Some((src, sel)) = which.split_once(':') else {
        return Ok(None);
    };
    let Some(set) = sets.iter().find(|set| set.source == src.trim()) else {
        return Ok(None);
    };
    let rules = set
        .rules
        .as_ref()
        .map_err(|why| format!("The rules of library {} cannot be read: {why}", set.source))?;
    let one = [which.to_owned()];
    let hits: Vec<(usize, &CheckRule)> = rules
        .iter()
        .enumerate()
        .map(|(k, r)| (k + 1, r))
        .filter(|&(n, r)| turned_off(&one, &set.source, n, r))
        .collect();
    if let [_, _, ..] = hits.as_slice() {
        let list: Vec<String> = hits.iter().map(|(n, _)| format!("{}:{n}", set.source)).collect();
        return Err(format!(
            "{which:?} could be rule {} of library {}: {:?} is a number and a rule's name too, or two rules share it. An entry in rules_off turns off all of them; name the rule another way (by its number).",
            list.join(" or "),
            set.source,
            sel.trim()
        ));
    }
    Ok(hits.first().map(|&(n, r)| (set.source.as_str(), n, r)))
}

/// `off` (a project's [`ProjectMeta::rules_off`]) with the library rule `which` turned off
/// (`turn_off`) or back on: `Ok(None)` when it is that way already. Turning a rule off adds
/// `which` as written; turning it on drops every entry that turns it off, by number or by
/// name. An entry naming no rule (a source since removed) can still be dropped.
///
/// [`ProjectMeta::rules_off`]: crate::project::ProjectMeta::rules_off
pub fn turn_rule(off: &[String], sets: &[RuleSet], which: &str, turn_off: bool) -> Result<Option<Vec<String>>, String> {
    let which = which.trim();
    // A source whose rules cannot be read: turning one of its rules off is refused with why,
    // but an entry for it can still be dropped as written.
    let unreadable = which
        .split_once(':')
        .is_some_and(|(src, _)| sets.iter().any(|set| set.source == src.trim() && set.rules.is_err()));
    let found = match library_rule(sets, which) {
        Err(_) if !turn_off && unreadable => None,
        found => found?,
    };
    let mut now = off.to_vec();
    match (turn_off, found) {
        (true, None) => {
            return Err(format!(
                "No library check rule {which:?}: write source:number or source:name."
            ));
        }
        (true, Some((src, n, r))) => {
            if turned_off(&now, src, n, r) {
                return Ok(None);
            }
            now.push(which.to_owned());
        }
        (false, Some((src, n, r))) => now.retain(|o| !turned_off(std::slice::from_ref(o), src, n, r)),
        (false, None) => now.retain(|o| o.trim() != which),
    }
    Ok(Some(now).filter(|now| now != off))
}

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

    #[test]
    fn a_rule_is_kept_as_the_check_runs_it_or_refused_with_why() {
        let rule = |kind: &str| CheckRule {
            kind: kind.into(),
            ..CheckRule::default()
        };
        let cable = checked(
            &CheckRule {
                role: Some(" Terminal ".into()),
                pattern: Some("-X#".into()),
                max: Some(3),
                message: Some("  ".into()),
                ..rule(" CABLE ")
            },
            1,
        )
        .unwrap();
        // Lower case kind, trimmed values, and values only another kind uses dropped.
        assert_eq!(cable.kind, "cable");
        assert_eq!(cable.role.as_deref(), Some("Terminal"));
        assert_eq!((cable.pattern, cable.max, cable.message), (None, None, None));
        assert!(
            checked(&rule("wires"), 4)
                .unwrap_err()
                .starts_with("Rule 4: pick a kind")
        );
        assert!(checked(&rule("tag"), 1).unwrap_err().contains("needs the pattern"));
        assert!(checked(&rule("max-wires"), 1).unwrap_err().contains("at most"));
        let bad_severity = CheckRule {
            severity: Some("fatal".into()),
            ..rule("cable")
        };
        assert!(
            checked(&bad_severity, 2)
                .unwrap_err()
                .starts_with("Rule 2: pick a severity")
        );
        // Unknown keys: kept by `checked` (a loaded rule saves them back), refused for a new one.
        let mut misspelt = rule("cable");
        misspelt
            .unknown
            .insert("severty".into(), toml::Value::String("error".into()));
        assert_eq!(checked(&misspelt, 1).unwrap().unknown.len(), 1);
        assert!(
            check_rule(&misspelt, 3)
                .unwrap_err()
                .starts_with("Rule 3: no such key severty")
        );
    }

    #[test]
    fn a_library_rule_is_turned_off_and_on_by_number_or_name() {
        let named = |name: &str| CheckRule {
            name: Some(name.into()),
            kind: "cable".into(),
            ..CheckRule::default()
        };
        let sets = [
            RuleSet {
                source: "house".into(),
                rules: Ok(vec![named("motors"), named("2"), named("dup"), named("dup")]),
            },
            RuleSet {
                source: "broken".into(),
                rules: Err("bad toml".into()),
            },
        ];
        let off = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            library_rule(&sets, " house:motors ").unwrap().map(|(s, n, _)| (s, n)),
            Some(("house", 1))
        );
        assert_eq!(library_rule(&sets, "house:9").unwrap(), None);
        assert!(library_rule(&sets, "broken:1").unwrap_err().contains("bad toml"));
        assert!(
            turn_rule(&[], &sets, "broken:1", true)
                .unwrap_err()
                .contains("bad toml")
        );
        assert_eq!(
            turn_rule(&off(&["broken:1"]), &sets, "broken:1", false).unwrap(),
            Some(vec![])
        );
        assert_eq!(library_rule(&sets, "motors").unwrap(), None);
        // Rule 2 by number and rule 2 named "2" are one rule; two of one name are not.
        assert!(library_rule(&sets, "house:2").unwrap().is_some());
        assert!(
            library_rule(&sets, "house:dup")
                .unwrap_err()
                .contains("house:3 or house:4")
        );

        assert_eq!(turn_rule(&[], &sets, "house:1", true).unwrap(), Some(off(&["house:1"])));
        assert_eq!(
            turn_rule(&off(&["house:motors"]), &sets, "house:1", true).unwrap(),
            None
        );
        assert!(turn_rule(&[], &sets, "house:9", true).is_err());
        // On again drops every entry that turns it off, by number or name.
        assert_eq!(
            turn_rule(
                &off(&["house:1", "house:motors", "house:2"]),
                &sets,
                "house:motors",
                false
            )
            .unwrap(),
            Some(off(&["house:2"]))
        );
        assert_eq!(turn_rule(&off(&["house:2"]), &sets, "house:1", false).unwrap(), None);
        assert_eq!(
            turn_rule(&off(&["gone:4"]), &sets, "gone:4", false).unwrap(),
            Some(vec![])
        );
    }

    /// The assistant's `rules` tool and the command line both turn rules through
    /// [`turn_rule`]: a selector that is one rule's number and another's name is refused
    /// both ways and changes nothing, and turning either rule on drops an entry that turns
    /// off both.
    #[test]
    fn a_selector_that_is_one_rules_number_and_anothers_name_is_refused() {
        let named = |name: &str| CheckRule {
            name: Some(name.into()),
            kind: "cable".into(),
            ..CheckRule::default()
        };
        let sets = [RuleSet {
            source: "odd".into(),
            rules: Ok(vec![named("2"), named("tags")]),
        }];
        let off = vec!["odd:tags".to_owned()];
        for turn_off in [true, false] {
            let r = turn_rule(&off, &sets, "odd:2", turn_off);
            assert!(r.as_ref().is_err_and(|e| e.contains("odd:1 or odd:2")), "{r:?}");
        }
        // An entry already there that turns off both goes when either is turned on.
        let both = vec!["odd:2".to_owned()];
        assert_eq!(turn_rule(&both, &sets, "odd:1", false).unwrap(), Some(vec![]));
        assert_eq!(turn_rule(&both, &sets, "odd:tags", false).unwrap(), Some(vec![]));
        assert_eq!(turn_rule(&both, &sets, "odd:tags", true).unwrap(), None, "off already");
    }
}
