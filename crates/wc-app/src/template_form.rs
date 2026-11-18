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
        for (i, b) in draft.boxes.iter_mut().enumerate() {
            let heading = if !b.label.is_empty() {
                b.label.clone()
            } else if b.logo {
                "(logo)".to_owned()
            } else {
                "(no label)".to_owned()
            };
            egui::CollapsingHeader::new(heading).id_salt(("box", i)).show(ui, |ui| {
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
