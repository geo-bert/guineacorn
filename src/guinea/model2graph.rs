use crate::guinea::giraphe::Giraphe;
use crate::guinea::Guineacorn;
use crate::unicorn::btor2file_parser::parse_btor2_file;
use crate::unicorn::builder::generate_model;
use crate::unicorn::unroller::renumber_model;
use crate::unicorn::Node;
use bytesize::ByteSize;
use egui::Ui;
use riscu::load_object_file;
use std::path::PathBuf;
use std::str::FromStr;

pub fn input_window(data: &mut Guineacorn, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Select a BTOR2 file to start.");
        if ui.button("Open file...").clicked() {
            data.reset_model_params();
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                data.picked_path = Some(path.display().to_string());
            }
        }
    });

    let picked_path = data
        .picked_path
        .clone()
        .unwrap_or_else(|| "NONE".to_string());

    ui.add_space(10.0);
    ui.add_enabled_ui(data.picked_path.is_some(), |ui| {
        ui.label("Selected File:");
        ui.monospace(&picked_path);

        if ui.button("Load File").clicked() {
            data.reset_model_params();
            // TODO: do proper input validation
            let path = PathBuf::from_str(&picked_path).unwrap();
            let mut model = parse_btor2_file(&path);
            renumber_model(&mut model);
            data.giraphe = Giraphe::from(&model);
        }

        if ui.button("Load Binary").clicked() {
            let path = PathBuf::from_str(&picked_path).unwrap();
            let program = load_object_file(path);

            let program = program.unwrap();
            let argv = [vec![picked_path.clone()]].concat();
            let model = generate_model(&program, ByteSize::mib(1).as_u64(), 8, 32, &argv);

            data.giraphe = Giraphe::from(&model.unwrap());
        }

        ui.add_enabled_ui(!data.giraphe.leaves.is_empty(), |ui| {
            if ui.button("Step Over").clicked() {
                let t = data.giraphe.tick_over();
                println!("Tick: {t}");
            }
        });

        ui.label("Leaf values");
        egui::ScrollArea::vertical()
            .id_source("leaves")
            .show(ui, |ui| {
                for sr in &data.giraphe.states {
                    let l = &*sr.borrow();
                    //if l.val_cur != l.val_old {
                    let text = match &*l.origin.borrow() {
                        Node::State { name, .. } => {
                            format!("State ({}): {}", name.as_ref().unwrap(), l.val_cur)
                        }
                        x => unreachable!("{:?}", x),
                    };
                    ui.label(text);
                    //}
                }
            });
    });
}

pub fn output_window(data: &mut Guineacorn, ui: &mut Ui) {
    data.giraphe.draw(ui);
    data.giraphe.interact(ui);
}
