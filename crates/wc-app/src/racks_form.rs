//! The PLC racks form: the racks and the I/O cards in their slots ([`wc_core::plc_rack`]),
//! laid out on the PLC card overview.

use eframe::egui;
use wc_core::plc_rack::{MAX_CHANNELS, PlcCard, PlcRack};

/// A new, empty rack named after the first PLC tag (`-A1`, `-A2`, ...) no rack has yet.
pub fn new_rack(racks: &[PlcRack]) -> PlcRack {
    let name = (1..)
        .map(|n| format!("-A{n}"))
        .find(|n| racks.iter().all(|r| r.name.trim() != n))
        .unwrap_or_default();
    PlcRack {
        name,
        ..PlcRack::default()
    }
}

/// A new card in the slot after the rack's last one, with 16 channels. Its start address is
/// left for the user: the overview warns about it until it is filled in.
pub fn new_card(rack: &PlcRack) -> PlcCard {
    PlcCard {
        slot: rack.cards.iter().map(|c| c.slot + 1).max().unwrap_or(1),
        channels: 16,
        ..PlcCard::default()
    }
}

/// The id of the start address field of card `card` in rack `rack`.
pub fn start_id(rack: usize, card: usize) -> egui::Id {
    egui::Id::new(("rack-card-start", rack, card))
}

fn text(ui: &mut egui::Ui, value: &mut String, width: f32, hint: &str) -> bool {
    ui.add(egui::TextEdit::singleline(value).desired_width(width).hint_text(hint))
        .changed()
}

/// One card's row of the rack's grid; the second value is true when its remove button was
/// clicked.
fn card_row(ui: &mut egui::Ui, card: &mut PlcCard, at: (usize, usize)) -> (bool, bool) {
    let mut changed = ui.add(egui::DragValue::new(&mut card.slot).range(0..=999)).changed();
    changed |= text(ui, &mut card.tag, 80.0, "-A1.DI1");
    changed |= text(ui, &mut card.kind, 140.0, "DI 16x24 V DC");
    changed |= ui
        .add(egui::DragValue::new(&mut card.channels).range(0..=MAX_CHANNELS))
        .changed();
    changed |= ui
        .add(
            egui::TextEdit::singleline(&mut card.start)
                .id(start_id(at.0, at.1))
                .desired_width(80.0)
                .hint_text("%IX0.0"),
        )
        .changed();
    ui.horizontal(|ui| {
        let mut own = card.step.is_some();
        if ui
            .checkbox(&mut own, "")
            .on_hover_text("Number the channels with a step of their own, for PLCs that count words one by one")
            .changed()
        {
            card.step = own.then_some(1);
            changed = true;
        }
        if let Some(step) = card.step.as_mut() {
            changed |= ui.add(egui::DragValue::new(step).range(1..=64)).changed();
        }
    });
    let remove = ui.small_button("×").on_hover_text("Remove this card").clicked();
    (changed || remove, remove)
}

/// Draw the form for `racks`; true when the user changed them.
pub fn show(ui: &mut egui::Ui, racks: &mut Vec<PlcRack>) -> bool {
    let mut changed = false;
    let mut drop_rack = None;
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        if racks.is_empty() {
            ui.label("No PLC racks yet.");
        }
        for (ri, rack) in racks.iter_mut().enumerate() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Rack");
                    changed |= text(ui, &mut rack.name, 80.0, "-A1");
                    ui.label("Description");
                    changed |= text(ui, &mut rack.description, 180.0, "Main PLC");
                    if ui.button("Remove rack").clicked() {
                        drop_rack = Some(ri);
                    }
                });
                let mut drop_card = None;
                if !rack.cards.is_empty() {
                    egui::Grid::new(("rack-cards", ri))
                        .num_columns(7)
                        .spacing([6.0, 4.0])
                        .show(ui, |ui| {
                            for h in ["Slot", "Tag", "Type", "Channels", "Start address", "Step", ""] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            for (ci, card) in rack.cards.iter_mut().enumerate() {
                                let (c, remove) = card_row(ui, card, (ri, ci));
                                changed |= c;
                                if remove {
                                    drop_card = Some(ci);
                                }
                                ui.end_row();
                            }
                        });
                }
                if let Some(ci) = drop_card {
                    rack.cards.remove(ci);
                }
                if ui.button("Add card").clicked() {
                    let card = new_card(rack);
                    rack.cards.push(card);
                    changed = true;
                }
            });
        }
    });
    if let Some(ri) = drop_rack {
        racks.remove(ri);
        changed = true;
    }
    if ui.button("Add rack").clicked() {
        let rack = new_rack(racks);
        racks.push(rack);
        changed = true;
    }
    changed
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_new_rack_takes_the_first_free_name_and_a_new_card_the_next_slot() {
        let mut racks = vec![new_rack(&[])];
        assert_eq!(racks[0].name, "-A1");
        racks.push(new_rack(&racks));
        assert_eq!(racks[1].name, "-A2");
        racks.remove(0);
        assert_eq!(new_rack(&racks).name, "-A1");

        let mut rack = PlcRack::default();
        assert_eq!(new_card(&rack).slot, 1);
        rack.cards.push(PlcCard {
            slot: 4,
            ..PlcCard::default()
        });
        let card = new_card(&rack);
        assert_eq!((card.slot, card.channels), (5, 16));
    }

    /// Run the form for one frame with `events`, as the app does.
    fn frame(ctx: &egui::Context, racks: &mut Vec<PlcRack>, events: Vec<egui::Event>) -> bool {
        let mut changed = false;
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| changed = show(ui, racks));
        });
        changed
    }

    #[test]
    fn typing_into_a_card_changes_it() {
        let mut racks = vec![PlcRack {
            name: "-A1".into(),
            cards: vec![new_card(&PlcRack::default())],
            ..PlcRack::default()
        }];
        let ctx = egui::Context::default();
        assert!(!frame(&ctx, &mut racks, vec![]), "nothing changed by drawing it");
        ctx.memory_mut(|m| m.request_focus(start_id(0, 0)));
        frame(&ctx, &mut racks, vec![]);
        for t in ["%IX", "0.0"] {
            assert!(frame(&ctx, &mut racks, vec![egui::Event::Text(t.into())]));
        }
        assert_eq!(racks[0].cards[0].start, "%IX0.0");
    }
}
