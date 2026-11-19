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
            kind: rule.kind.clone(),
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
        let kind = self.kind.trim();
        if !KINDS.contains(&kind) {
            return Err(format!("Rule {n}: pick a kind ({}).", KINDS.join(", ")));
        }
        let max = match set(&self.max) {
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
        Ok(CheckRule {
            kind: kind.to_owned(),
            devices: set(&self.devices),
            role: set(&self.role),
            // Only a tag rule uses a pattern, and only a max-wires rule a maximum.
            pattern: if kind == "tag" { pattern } else { None },
            max: if kind == "max-wires" { max } else { None },
            severity: set(&self.severity),
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
enum Change {
    Up(usize),
    Down(usize),
    Remove(usize),
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
    let mut change = None;
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
                            change = Some(Change::Up(k));
                        }
                        if ui.add_enabled(k < last, egui::Button::new("Down")).clicked() {
                            change = Some(Change::Down(k));
                        }
                        if ui.button("Delete").clicked() {
                            change = Some(Change::Remove(k));
                        }
                    });
                    egui::Grid::new("rule")
                        .num_columns(2)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Role");
                            choice(ui, &mut row.role, &ROLES, Some("any"), "role");
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
    match change {
        Some(Change::Up(k)) => rows.swap(k - 1, k),
        Some(Change::Down(k)) => rows.swap(k, k + 1),
        Some(Change::Remove(k)) => {
            rows.remove(k);
        }
        None => {}
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
    fn values_another_kind_uses_are_not_written() {
        let mut row = RuleRow::new("max-wires");
        row.pattern = "-X*".into();
        row.kind = "cable".into();
        let rule = row.rule(1).unwrap();
        assert_eq!((rule.pattern, rule.max), (None, None));
    }
}
