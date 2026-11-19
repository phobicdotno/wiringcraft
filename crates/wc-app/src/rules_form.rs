//! The check rules dialog: the project's `[[rule]]` tables ([`wc_core::check_rule`]) as rows to
//! add, change, reorder and delete, next to the check list.

use eframe::egui;
use wc_core::check_rule::{CheckRule, KINDS};

/// The roles a rule can name, as written in `role`.
pub const ROLES: [&str; 5] = ["main", "auxiliary", "standalone", "terminal", "interruption"];
/// The severities a rule can have, as written in `severity`.
pub const SEVERITIES: [&str; 3] = ["error", "warning", "note"];

/// One rule as the dialog edits it: each value as text, empty for "not set".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleRow {
    pub kind: String,
    pub role: String,
    pub devices: String,
    pub pattern: String,
    pub max: String,
    pub severity: String,
    pub message: String,
    /// The rule as loaded: keys the dialog does not show (a misspelt `severty`) are written
    /// back from it.
    pub base: CheckRule,
}

impl RuleRow {
    /// The row of a rule.
    pub fn of(rule: &CheckRule) -> Self {
        let text = |v: &Option<String>| v.clone().unwrap_or_default();
        RuleRow {
            // As the check reads it, so the row shows that kind's fields.
            kind: rule.kind.trim().to_ascii_lowercase(),
            role: text(&rule.role),
            devices: text(&rule.devices),
            pattern: text(&rule.pattern),
            max: rule.max.map(|m| m.to_string()).unwrap_or_default(),
            severity: text(&rule.severity),
            message: text(&rule.message),
            base: rule.clone(),
        }
    }

    /// A new rule of `kind`.
    pub fn new(kind: &str) -> Self {
        let mut row = RuleRow {
            kind: kind.to_owned(),
            ..RuleRow::default()
        };
        if kind == "max-wires" {
            row.max = "2".into();
        }
        row
    }

    /// The rule this row stands for; `n` is its number in the list (from 1), for the error.
    pub fn rule(&self, n: usize) -> Result<CheckRule, String> {
        let set = |v: &str| Some(v.trim().to_owned()).filter(|v| !v.is_empty());
        // Any case, as the check reads it.
        let kind = self.kind.trim().to_ascii_lowercase();
        let kind = kind.as_str();
        if !KINDS.contains(&kind) {
            return Err(format!("Rule {n}: pick a kind ({}).", KINDS.join(", ")));
        }
        // Only a max-wires rule uses a maximum: one typed before the kind changed is left.
        let max = match set(&self.max).filter(|_| kind == "max-wires") {
            None => None,
            Some(m) => Some(
                m.parse::<u32>()
                    .map_err(|_| format!("Rule {n}: the most wires must be a whole number, not \"{m}\"."))?,
            ),
        };
        if kind == "max-wires" && max.is_none() {
            return Err(format!("Rule {n}: say how many wires a pin may have at most."));
        }
        let pattern = set(&self.pattern);
        if kind == "tag" && pattern.is_none() {
            return Err(format!("Rule {n}: a tag rule needs the pattern every tag must match."));
        }
        // The check runs no rule with another role or severity (it reports it instead).
        let known = |v: &Option<String>, of: &[&str]| {
            v.as_deref()
                .is_none_or(|v| of.contains(&v.to_ascii_lowercase().as_str()))
        };
        let (role, severity) = (set(&self.role), set(&self.severity));
        if !known(&role, &ROLES) {
            return Err(format!("Rule {n}: pick a role ({}), or none.", ROLES.join(", ")));
        }
        if !known(&severity, &SEVERITIES) {
            return Err(format!("Rule {n}: pick a severity ({}).", SEVERITIES.join(", ")));
        }
        Ok(CheckRule {
            name: self.base.name.clone(),
            kind: kind.to_owned(),
            devices: set(&self.devices),
            role,
            // Only a tag rule uses a pattern.
            pattern: if kind == "tag" { pattern } else { None },
            max,
            severity,
            message: set(&self.message),
            unknown: self.base.unknown.clone(),
        })
    }
}

/// The rows of `rules`, in order.
pub fn rows_of(rules: &[CheckRule]) -> Vec<RuleRow> {
    rules.iter().map(RuleRow::of).collect()
}

/// The rules of `rows`, in order, or why one cannot be kept.
pub fn rules_of(rows: &[RuleRow]) -> Result<Vec<CheckRule>, String> {
    rows.iter().enumerate().map(|(k, r)| r.rule(k + 1)).collect()
}

/// What a click in the list asks for.
#[derive(Debug, Clone, Copy)]
pub enum Change {
    Up(usize),
    Down(usize),
    Remove(usize),
}

/// Move a row up or down, or delete it; a move past either end does nothing.
pub fn change(rows: &mut Vec<RuleRow>, c: Change) {
    match c {
        Change::Up(k) if k > 0 && k < rows.len() => rows.swap(k - 1, k),
        Change::Down(k) if k + 1 < rows.len() => rows.swap(k, k + 1),
        Change::Remove(k) if k < rows.len() => {
            rows.remove(k);
        }
        _ => {}
    }
}

/// What the role list says when no role is picked: a max-wires rule without devices still
/// looks at terminals only.
pub fn unset_role(row: &RuleRow) -> &'static str {
    if row.kind.trim() == "max-wires" && row.devices.trim().is_empty() {
        "terminal (when no devices are given)"
    } else {
        "any"
    }
}

fn choice(ui: &mut egui::Ui, value: &mut String, options: &[&str], unset: Option<&str>, salt: impl std::hash::Hash) {
    let shown = if value.trim().is_empty() {
        unset.unwrap_or("").to_owned()
    } else {
        value.clone()
    };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if let Some(u) = unset {
                ui.selectable_value(value, String::new(), u);
            }
            for o in options {
                ui.selectable_value(value, (*o).to_owned(), *o);
            }
        });
}

/// The rows as a list with their fields, plus buttons to add a rule.
pub fn show(ui: &mut egui::Ui, rows: &mut Vec<RuleRow>) {
    let faint = crate::theme::Theme::of(ui.ctx()).faint;
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("rules-scroll")
        .max_height(ui.ctx().screen_rect().height() * 0.5)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if rows.is_empty() {
                ui.label(egui::RichText::new("No rules yet. Add one below.").color(faint));
            }
            let last = rows.len().saturating_sub(1);
            for (k, row) in rows.iter_mut().enumerate() {
                ui.push_id(k, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Rule {}", k + 1)).strong());
                        choice(ui, &mut row.kind, &KINDS, None, "kind");
                        if ui.add_enabled(k > 0, egui::Button::new("Up")).clicked() {
                            clicked = Some(Change::Up(k));
                        }
                        if ui.add_enabled(k < last, egui::Button::new("Down")).clicked() {
                            clicked = Some(Change::Down(k));
                        }
                        if ui.button("Delete").clicked() {
                            clicked = Some(Change::Remove(k));
                        }
                    });
                    egui::Grid::new("rule")
                        .num_columns(2)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Role");
                            let unset = unset_role(row);
                            choice(ui, &mut row.role, &ROLES, Some(unset), "role");
                            ui.end_row();
                            ui.label("Devices");
                            ui.add(
                                egui::TextEdit::singleline(&mut row.devices)
                                    .hint_text("all, or a tag pattern like -M*")
                                    .desired_width(260.0),
                            );
                            ui.end_row();
                            match row.kind.trim() {
                                "tag" => {
                                    ui.label("Pattern");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut row.pattern)
                                            .hint_text("-X#.#")
                                            .desired_width(260.0),
                                    );
                                    ui.end_row();
                                }
                                "max-wires" => {
                                    ui.label("Most wires on a pin");
                                    ui.add(egui::TextEdit::singleline(&mut row.max).desired_width(60.0));
                                    ui.end_row();
                                }
                                _ => {}
                            }
                            ui.label("Severity");
                            choice(
                                ui,
                                &mut row.severity,
                                &SEVERITIES,
                                Some("warning (default)"),
                                "severity",
                            );
                            ui.end_row();
                            ui.label("Message");
                            ui.add(
                                egui::TextEdit::singleline(&mut row.message)
                                    .hint_text("the built-in text; {tag} stands for the tag")
                                    .desired_width(260.0),
                            );
                            ui.end_row();
                        });
                    ui.separator();
                });
            }
        });
    if let Some(c) = clicked {
        change(rows, c);
    }
    ui.horizontal(|ui| {
        ui.label("Add a rule:");
        for (kind, what) in [
            ("cable", "Devices have a cable"),
            ("tag", "Tags match a pattern"),
            ("max-wires", "Wires on a pin"),
        ] {
            if ui.button(what).clicked() {
                rows.push(RuleRow::new(kind));
            }
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_comes_back_from_its_row_with_the_keys_the_dialog_does_not_show() {
        let mut rule = CheckRule {
            kind: "tag".into(),
            role: Some("terminal".into()),
            pattern: Some("-X#.#".into()),
            severity: Some("error".into()),
            ..CheckRule::default()
        };
        rule.unknown
            .insert("severty".into(), toml::Value::String("note".into()));
        assert_eq!(RuleRow::of(&rule).rule(1), Ok(rule));
    }

    #[test]
    fn a_row_that_cannot_be_a_rule_says_which_and_why() {
        let rows = vec![RuleRow::new("cable"), RuleRow::new("tag")];
        let err = rules_of(&rows).err().unwrap();
        assert!(err.starts_with("Rule 2:") && err.contains("pattern"), "{err}");
        let mut max = RuleRow::new("max-wires");
        max.max = "two".into();
        assert!(max.rule(1).err().unwrap().contains("whole number"));
        max.max = String::new();
        assert!(max.rule(1).is_err());
        assert!(RuleRow::default().rule(1).err().unwrap().contains("kind"));
    }

    #[test]
    fn a_maximum_typed_before_the_kind_changed_does_not_stop_the_rule() {
        let mut row = RuleRow::new("max-wires");
        row.max = "two".into();
        assert!(row.rule(1).is_err());
        row.kind = "cable".into();
        assert_eq!(row.rule(1).map(|r| r.max), Ok(None));
    }

    #[test]
    fn a_role_or_severity_the_check_does_not_know_is_refused_and_any_case_of_one_it_does_kept() {
        let mut row = RuleRow::new("cable");
        row.role = "bogus".into();
        assert!(row.rule(3).err().unwrap().starts_with("Rule 3: pick a role"));
        row.role = "Terminal".into();
        row.severity = "fatal".into();
        assert!(row.rule(3).err().unwrap().starts_with("Rule 3: pick a severity"));
        row.severity = "Error".into();
        let rule = row.rule(3).unwrap();
        assert_eq!(
            (rule.role.as_deref(), rule.severity.as_deref()),
            (Some("Terminal"), Some("Error"))
        );
    }

    #[test]
    fn rows_move_up_and_down_and_are_deleted() {
        let rows_of_kinds = |kinds: &[&str]| kinds.iter().map(|k| RuleRow::new(k)).collect::<Vec<_>>();
        let mut rows = rows_of_kinds(&["cable", "tag", "max-wires"]);
        change(&mut rows, Change::Up(2));
        assert_eq!(rows, rows_of_kinds(&["cable", "max-wires", "tag"]));
        change(&mut rows, Change::Down(0));
        assert_eq!(rows, rows_of_kinds(&["max-wires", "cable", "tag"]));
        change(&mut rows, Change::Up(0));
        change(&mut rows, Change::Down(2));
        assert_eq!(
            rows,
            rows_of_kinds(&["max-wires", "cable", "tag"]),
            "nothing moves past an end"
        );
        change(&mut rows, Change::Remove(1));
        assert_eq!(rows, rows_of_kinds(&["max-wires", "tag"]));
    }

    #[test]
    fn a_max_wires_rule_without_devices_says_it_looks_at_terminals() {
        let mut row = RuleRow::new("max-wires");
        assert!(unset_role(&row).starts_with("terminal"));
        row.devices = "-X1*".into();
        assert_eq!(unset_role(&row), "any");
        assert_eq!(unset_role(&RuleRow::new("cable")), "any");
    }

    #[test]
    fn a_kind_in_another_case_is_kept_as_the_check_reads_it() {
        let rule = CheckRule {
            kind: "TAG".into(),
            pattern: Some("-X*".into()),
            ..CheckRule::default()
        };
        assert_eq!(RuleRow::of(&rule).rule(1).map(|r| r.kind), Ok("tag".into()));
        let row = RuleRow::of(&CheckRule {
            kind: " MAX-WIRES".into(),
            ..CheckRule::default()
        });
        assert_eq!(row.kind, "max-wires", "the row shows the maximum field");
        assert!(unset_role(&row).starts_with("terminal"));
    }

    #[test]
    fn values_another_kind_uses_are_not_written() {
        let mut row = RuleRow::new("max-wires");
        row.pattern = "-X*".into();
        row.kind = "cable".into();
        let rule = row.rule(1).unwrap();
        assert_eq!((rule.pattern, rule.max), (None, None));
    }
}
