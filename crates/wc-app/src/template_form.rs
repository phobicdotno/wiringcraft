//! The page template form: the frame's column count and the title block's boxes.

use eframe::egui;
use wc_core::geom::Align;
use wc_core::template::{self, BoxText, MAX_COLUMNS, MAX_TITLE_HEIGHT, MIN_TITLE_HEIGHT, PageTemplate, Side, TitleBox};

/// The values a title-block text can show: the special ones, the standard title-block fields
/// and any other field the project has, each once.
pub fn value_keys(project: &wc_core::Project) -> Vec<String> {
    let mut keys: Vec<String> = template::SPECIAL_VALUES.iter().map(|(k, _)| (*k).to_owned()).collect();
    keys.extend(wc_core::project::TITLE_FIELDS.iter().map(|(k, _)| (*k).to_owned()));
    for k in project.meta.title_block.keys() {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    keys
}

/// Carry the boxes' open or closed headers along when the boxes move: `ids` are the headers
/// by box index and `op` does to their open flags what is done to the boxes, so the box that
/// was drawn earlier or the ones after a removed box keep their header open, not the box that
/// takes their place in the list.
fn carry_open(ctx: &egui::Context, ids: &[egui::Id], op: impl FnOnce(&mut Vec<bool>)) {
    use egui::collapsing_header::CollapsingState;
    let mut open: Vec<bool> = ids
        .iter()
        .map(|&id| CollapsingState::load(ctx, id).is_some_and(|s| s.is_open()))
        .collect();
    op(&mut open);
    for (k, &id) in ids.iter().enumerate() {
        let mut s = CollapsingState::load_with_default_open(ctx, id, false);
        s.set_open(open.get(k).copied().unwrap_or(false));
        s.store(ctx);
    }
}

fn edge(ui: &mut egui::Ui, mm: &mut f64, from: &mut Side, salt: impl std::hash::Hash) -> bool {
    let mut changed = ui
        .add(egui::DragValue::new(mm).range(0.0..=600.0).speed(0.5).suffix(" mm"))
        .changed();
    egui::ComboBox::from_id_salt(salt)
        .selected_text(match from {
            Side::Left => "from the left",
            Side::Right => "from the right",
        })
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(from, Side::Left, "from the left").changed();
            changed |= ui.selectable_value(from, Side::Right, "from the right").changed();
        });
    changed
}

fn text_row(ui: &mut egui::Ui, t: &mut BoxText, keys: &[String], band: f64, salt: (usize, usize)) -> (bool, bool) {
    let mut changed = false;
    let mut remove_text = false;
    ui.horizontal_wrapped(|ui| {
        let mut drop = None;
        let several = t.values.len() > 1;
        for (k, v) in t.values.iter_mut().enumerate() {
            egui::ComboBox::from_id_salt(("value", salt, k))
                .selected_text(template::value_label(v))
                .show_ui(ui, |ui| {
                    for key in keys {
                        changed |= ui
                            .selectable_value(v, key.clone(), template::value_label(key))
                            .changed();
                    }
                });
            if several && ui.small_button("×").on_hover_text("Leave this value out").clicked() {
                drop = Some(k);
            }
        }
        if let Some(k) = drop {
            t.values.remove(k);
            changed = true;
        }
        if ui
            .small_button("+")
            .on_hover_text("Show one more value here, after a dot")
            .clicked()
        {
            t.values.push(keys.first().cloned().unwrap_or_default());
            changed = true;
        }
        ui.label("size");
        changed |= ui
            .add(egui::DragValue::new(&mut t.size).range(0.5..=12.0).speed(0.1))
            .changed();
        ui.label("at");
        changed |= ui
            .add(
                egui::DragValue::new(&mut t.y)
                    .range(0.0..=band)
                    .speed(0.1)
                    .suffix(" mm"),
            )
            .on_hover_text("Baseline, below the box's top")
            .changed();
        egui::ComboBox::from_id_salt(("align", salt))
            .selected_text(match t.align {
                Align::Left => "left",
                Align::Center => "centred",
                Align::Right => "right",
            })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut t.align, Align::Left, "left").changed();
                changed |= ui.selectable_value(&mut t.align, Align::Center, "centred").changed();
                changed |= ui.selectable_value(&mut t.align, Align::Right, "right").changed();
            });
        if t.align != Align::Center {
            ui.label("inset");
            changed |= ui
                .add(egui::DragValue::new(&mut t.inset).range(0.0..=50.0).speed(0.1))
                .changed();
        }
        remove_text = ui.small_button("Remove").on_hover_text("Remove this text").clicked();
    });
    (changed || remove_text, remove_text)
}

/// Draw the form for `draft`; true when the user changed it.
pub fn show(ui: &mut egui::Ui, draft: &mut PageTemplate, keys: &[String]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Reference columns");
        changed |= ui
            .add(egui::DragValue::new(&mut draft.columns).range(1..=MAX_COLUMNS))
            .on_hover_text("Across the frame; cross-references and wire numbers by column use them too")
            .changed();
        ui.label("Title block height");
        changed |= ui
            .add(
                egui::DragValue::new(&mut draft.height)
                    .range(MIN_TITLE_HEIGHT..=MAX_TITLE_HEIGHT)
                    .speed(0.5)
                    .suffix(" mm"),
            )
            .on_hover_text("A taller title block leaves less drawing area on every page")
            .changed();
    });
    let band = draft.band_height();
    ui.separator();
    ui.label(
        format!("Title block boxes (sides in mm from the frame's left or right edge, top and bottom in mm down the {band} mm band)"),
    );
    let mut remove = None;
    let mut raise = None;
    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
        // The id each header keeps its open state under, as the header itself reports it.
        let mut header_ids = Vec::with_capacity(draft.boxes.len());
        for (i, b) in draft.boxes.iter_mut().enumerate() {
            let heading = if !b.label.is_empty() {
                b.label.clone()
            } else if b.logo {
                "(logo)".to_owned()
            } else {
                "(no label)".to_owned()
            };
            let header = egui::CollapsingHeader::new(heading).id_salt(("box", i)).show(ui, |ui| {
                egui::Grid::new(("box-grid", i))
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Label");
                        changed |= ui.text_edit_singleline(&mut b.label).changed();
                        ui.end_row();
                        ui.label("Left side");
                        ui.horizontal(|ui| changed |= edge(ui, &mut b.left, &mut b.left_from, ("l", i)));
                        ui.end_row();
                        ui.label("Right side");
                        ui.horizontal(|ui| changed |= edge(ui, &mut b.right, &mut b.right_from, ("r", i)));
                        ui.end_row();
                        ui.label("Top, bottom");
                        ui.horizontal(|ui| {
                            for v in [&mut b.top, &mut b.bottom] {
                                changed |= ui
                                    .add(egui::DragValue::new(v).range(0.0..=band).speed(0.5).suffix(" mm"))
                                    .changed();
                            }
                        });
                        ui.end_row();
                        ui.label("Logo");
                        changed |= ui.checkbox(&mut b.logo, "the project's logo goes here").changed();
                        ui.end_row();
                    });
                let mut drop = None;
                for (j, t) in b.texts.iter_mut().enumerate() {
                    let (c, r) = text_row(ui, t, keys, band, (i, j));
                    changed |= c;
                    if r {
                        drop = Some(j);
                    }
                }
                if let Some(j) = drop {
                    b.texts.remove(j);
                }
                ui.horizontal(|ui| {
                    if ui.button("Add text").clicked() {
                        b.texts.push(BoxText::new(&["customer"], 2.5, 9.5, Align::Left));
                        changed = true;
                    }
                    if i > 0
                        && ui
                            .button("Draw earlier")
                            .on_hover_text("Move this box up the list")
                            .clicked()
                    {
                        raise = Some(i);
                    }
                    if ui.button("Remove box").clicked() {
                        remove = Some(i);
                    }
                });
            });
            header_ids.push(header.header_response.id);
        }
        if remove.is_some() || raise.is_some() {
            carry_open(ui.ctx(), &header_ids, |open| {
                if let Some(i) = remove {
                    open.remove(i);
                } else if let Some(i) = raise {
                    open.swap(i - 1, i);
                }
            });
        }
    });
    if let Some(i) = remove {
        draft.boxes.remove(i);
        changed = true;
    }
    if let Some(i) = raise {
        draft.boxes.swap(i - 1, i);
        changed = true;
    }
    ui.horizontal(|ui| {
        if ui.button("Add box").clicked() {
            let mut b = TitleBox::new("New box", (0.0, Side::Left), (40.0, Side::Left), 0.0, band / 2.0);
            b.texts.push(BoxText::new(&["customer"], 2.5, 9.5, Align::Left));
            draft.boxes.push(b);
            changed = true;
        }
        if !draft.is_default()
            && ui
                .button("Built-in layout")
                .on_hover_text("Put the columns and title block back as WiringCraft draws them")
                .clicked()
        {
            *draft = PageTemplate::default();
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(ctx: &egui::Context, ids: &[egui::Id]) -> Vec<bool> {
        ids.iter()
            .map(|&id| egui::collapsing_header::CollapsingState::load(ctx, id).is_some_and(|s| s.is_open()))
            .collect()
    }

    fn standard() -> Vec<String> {
        template::SPECIAL_VALUES
            .iter()
            .chain(wc_core::project::TITLE_FIELDS)
            .map(|(k, _)| (*k).to_owned())
            .collect()
    }

    #[test]
    fn an_open_box_stays_open_when_it_moves_or_one_before_it_is_removed() {
        let ctx = egui::Context::default();
        let ids: Vec<egui::Id> = (0..3).map(|i| egui::Id::new(("box", i))).collect();
        // Only the last box is open.
        carry_open(&ctx, &ids, |o| o[2] = true);
        assert_eq!(open(&ctx, &ids), [false, false, true]);
        // "Draw earlier" on it: it is box 2 now, still open, and the one it passed is closed.
        carry_open(&ctx, &ids, |o| o.swap(1, 2));
        assert_eq!(open(&ctx, &ids), [false, true, false]);
        // Removing the first box moves it to box 1, still open; the freed last slot is closed.
        carry_open(&ctx, &ids, |o| {
            o.remove(0);
        });
        assert_eq!(open(&ctx, &ids), [true, false, false]);
    }

    #[test]
    fn a_new_project_offers_the_special_values_then_the_title_fields_in_order() {
        let keys = value_keys(&wc_core::Project::new("x"));
        assert_eq!(keys, standard());
        assert_eq!(keys.first().map(String::as_str), Some(template::PROJECT_NAME));
    }

    #[test]
    fn the_projects_own_fields_come_after_the_standard_ones_in_name_order() {
        let mut p = wc_core::Project::new("x");
        p.meta.title_block.insert("zone".into(), "B".into());
        p.meta.title_block.insert("approved_by".into(), "Ann".into());
        let keys = value_keys(&p);
        let mut want = standard();
        want.extend(["approved_by".to_owned(), "zone".to_owned()]);
        assert_eq!(keys, want);
    }

    #[test]
    fn a_field_the_project_fills_that_is_already_standard_is_listed_once() {
        let mut p = wc_core::Project::new("x");
        p.meta.title_block.insert("customer".into(), "Acme".into());
        p.meta.title_block.insert(template::PAGE_TITLE.into(), "odd".into());
        let keys = value_keys(&p);
        assert_eq!(keys, standard());
        assert_eq!(keys.iter().filter(|k| *k == "customer").count(), 1);
        assert_eq!(keys.iter().filter(|k| *k == template::PAGE_TITLE).count(), 1);
    }

    #[test]
    fn every_key_is_unique() {
        let mut p = wc_core::Project::new("x");
        for k in standard() {
            p.meta.title_block.insert(k, "v".into());
        }
        p.meta.title_block.insert("extra".into(), "v".into());
        let keys = value_keys(&p);
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len());
        assert_eq!(keys.last().map(String::as_str), Some("extra"));
    }
}
