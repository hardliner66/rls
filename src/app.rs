use std::{collections::HashMap, ops::Deref, sync::Arc};

use eframe::{
    egui::{self, Context, Key, Margin, TextStyle, Ui},
    epaint::{Color32, Stroke},
    CreationContext,
};
use emath::{pos2, vec2, Align2, Pos2, Rect, Vec2};

use crate::{
    board::{ActiveCircuitBoard, CircuitBoard, SelectedBoardItem},
    circuits::{self, CircuitPreview, CircuitPreviewImpl},
    time::Instant,
    ui::{Inventory, InventoryItem, InventoryItemGroup},
    vector::{Vec2f, Vector},
    BasicLoadingContext, DynStaticStr, PaintContext, PanAndZoom, PastePreview, RwLock,
    TileDrawBounds,
};

pub struct App {
    #[cfg(not(feature = "wasm"))]
    last_win_pos: Option<Pos2>,
    #[cfg(not(feature = "wasm"))]
    last_win_size: Vec2,

    pub pan_zoom: PanAndZoom,
    pub board: ActiveCircuitBoard,

    pub debug: bool,

    paste: Option<PastePreview>,
    inventory_items: Vec<InventoryItemGroup>,
    selected_id: Option<String>,
    circuit_previews: HashMap<String, Arc<CircuitPreview>>,
}

// TODO: fix coi sometimes not working by re-registering it and reloading
impl eframe::App for App {
    fn update(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        #[cfg(not(feature = "wasm"))]
        {
            let int_info = frame.info();
            if let Some(win_pos) = int_info.window_info.position {
                if let Some(last_win_pos) = self.last_win_pos {
                    let win_size = int_info.window_info.size;
                    if win_size != self.last_win_size {
                        let diff: Vec2f = (win_pos - last_win_pos).into();
                        self.pan_zoom.pos += diff / self.pan_zoom.scale;
                    }
                }
            }
            self.last_win_pos = int_info.window_info.position;
            self.last_win_size = int_info.window_info.size;
        }
        ctx.request_repaint();

        #[cfg(feature = "single_thread")]
        self.board.board.read().unwrap().states.update();
        cfg_if::cfg_if! {
            if #[cfg(all(not(web_sys_unstable_apis), feature = "wasm"))] {
                let paste = ctx
                    .input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::V))
                    .then(|| crate::io::GLOBAL_CLIPBOARD.lock().unwrap().clone())
                    .flatten();
            } else {
                let paste = ctx.input(|input| {
                    input
                        .events
                        .iter()
                        .find_map(|e| match e {
                            egui::Event::Paste(s) => Some(s),
                            _ => None,
                        })
                        .and_then(|p| ron::from_str::<crate::io::CopyPasteData>(p).ok())
                });
            }
        }

        if let Some(paste) = paste {
            self.paste = Some(PastePreview::new(
                paste,
                &BasicLoadingContext {
                    previews: &self.circuit_previews,
                },
            ));
            self.selected_id = Some("paste".to_owned());
        }

        if ctx.input(|input| input.key_pressed(Key::F9)) {
            self.debug = !self.debug;
        } else if ctx.input(|input| input.key_pressed(Key::F8)) {
            let board = self.board.board.clone();
            self.board = ActiveCircuitBoard::new(board, 0).unwrap();
        } else if ctx.input(|input| input.key_pressed(Key::F4)) {
            let state = &self.board.state;
            state.reset();
            state.update_everything();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ctx.style().as_ref()).inner_margin(Margin::same(0.0)))
            .show(ctx, |ui| {
                self.main_update(ui, ctx);

                let mut selected = self.selected_id.take();
                if ui.input(|input| input.key_pressed(Key::Escape)) {
                    selected = None;
                }

                ui.add(Inventory {
                    selected: &mut selected,
                    groups: &self.inventory_items,
                    item_size: [28.0, 28.0].into(),
                    item_margin: Margin::same(6.0),
                    margin: Margin::same(10.0),
                });

                match (selected.as_deref(), &self.paste) {
                    (Some("paste"), Some(_)) => (),
                    (Some("paste"), None) => selected = None,
                    (_, Some(_)) => self.paste = None,
                    _ => (),
                }
                self.selected_id = selected;
            });
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        let board = self.board.board.read().unwrap();
        let data = board.save();
        _storage.set_string("board", ron::to_string(&data).unwrap());

        let previews = crate::io::CircuitPreviewCollectionData(HashMap::from_iter(
            self.circuit_previews.iter().filter_map(|(ty, p)| p.save().map(|d| (Arc::<str>::from(ty.clone()).into(), d)))),
        );
        _storage.set_string("previews", ron::to_string(&previews).unwrap());
    }
}

static INVENTORY_CIRCUIT_ORDER: [&str; 10] = [
    "button", "or", "nor", "and", "nand", "xor", "xnor", "not", "pullup", "test",
];

impl App {
    pub fn create(cc: &CreationContext) -> Self {
        let previews = [
            Box::new(circuits::test::Preview {}) as Box<dyn CircuitPreviewImpl>,
            Box::new(circuits::button::Preview {}),
            Box::new(circuits::gates::gate::Preview {
                template: circuits::gates::or::TEMPLATE,
            }),
            Box::new(circuits::gates::gate::Preview {
                template: circuits::gates::nor::TEMPLATE,
            }),
            Box::new(circuits::gates::gate::Preview {
                template: circuits::gates::xor::TEMPLATE,
            }),
            Box::new(circuits::gates::gate::Preview {
                template: circuits::gates::xnor::TEMPLATE,
            }),
            Box::new(circuits::gates::gate::Preview {
                template: circuits::gates::and::TEMPLATE,
            }),
            Box::new(circuits::gates::gate::Preview {
                template: circuits::gates::nand::TEMPLATE,
            }),
            Box::new(circuits::gates::not::Preview {}),
            Box::new(circuits::pullup::Preview {}),
        ];
        let empty_map = HashMap::default();
        let empty_loading_context = BasicLoadingContext::<'_, DynStaticStr> { previews: &empty_map };
        let preview_data = cc.storage.and_then(|s| s.get_string("previews")).and_then(|s| ron::from_str::<crate::io::CircuitPreviewCollectionData>(&s).ok());
        let previews = HashMap::from_iter(
            previews
                .into_iter()
                .map(|p| {
                    let data = preview_data.as_ref().and_then(|d| d.0.get(p.type_name().deref()));
                    let p = match data {
                        Some(d) => CircuitPreview::load_with_data(p, d, &empty_loading_context),
                        None => CircuitPreview::from_impl(p),
                    };
                    (p.imp.type_name(), Arc::new(p))
                }),
        );

        let ctx = BasicLoadingContext {
            previews: &previews,
        };
        let shift = cc.egui_ctx.input(|input| input.modifiers.shift);
        let board = (!shift)
            .then_some(cc.storage)
            .flatten()
            .and_then(|s| s.get_string("board"))
            .and_then(|s| ron::from_str::<crate::io::CircuitBoardData>(&s).ok())
            .map(|d| CircuitBoard::load(&d, &ctx));

        Self::new(board, previews)
    }

    pub fn new(
        board: Option<Arc<RwLock<CircuitBoard>>>,
        previews: HashMap<DynStaticStr, Arc<CircuitPreview>>,
    ) -> Self {
        let board = board.unwrap_or_else(|| Arc::new(RwLock::new(CircuitBoard::new())));

        #[cfg(not(feature = "single_thread"))]
        board.read().unwrap().activate();

        let state_id = {
            let circuit_board = board.read().unwrap();
            let states = circuit_board.states.states().read().unwrap();
            let first_id = states
                .inner()
                .iter()
                .enumerate()
                .find(|(_, v)| v.is_some())
                .map(|(i, _)| i);
            drop(states);
            first_id.unwrap_or_else(|| circuit_board.states.create_state(board.clone()).0)
        };
        let inventory_group: Vec<_> = {
            use std::cmp::Ordering;

            let mut vec: Vec<_> = previews.keys().collect();
            vec.sort_by(|a, b| {
                let a_ind = INVENTORY_CIRCUIT_ORDER
                    .iter()
                    .enumerate()
                    .find_map(|s| (a == s.1).then_some(s.0));
                let b_ind = INVENTORY_CIRCUIT_ORDER
                    .iter()
                    .enumerate()
                    .find_map(|s| (b == s.1).then_some(s.0));

                match (a_ind, b_ind) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    (None, Some(_)) => Ordering::Less,
                    (Some(_), None) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                }
            });
            vec.into_iter()
                .filter_map(|id| {
                    previews.get(id).map(|preview| {
                        Box::new(crate::CircuitInventoryItem {
                            preview: preview.clone(),
                            id: preview.imp.type_name().deref().to_owned(),
                        }) as Box<dyn InventoryItem>
                    })
                })
                .collect()
        };
        Self {
            pan_zoom: PanAndZoom::new(0.0.into(), 16.0),

            #[cfg(not(feature = "wasm"))]
            last_win_pos: None,
            #[cfg(not(feature = "wasm"))]
            last_win_size: Default::default(),
            board: ActiveCircuitBoard::new(board, state_id).unwrap(),
            debug: false,

            selected_id: None,
            inventory_items: vec![
                InventoryItemGroup::SingleItem(Box::new(crate::SelectionInventoryItem {})),
                InventoryItemGroup::SingleItem(Box::new(crate::WireInventoryItem {})),
                InventoryItemGroup::Group(inventory_group),
            ],
            circuit_previews: previews
                .into_iter()
                .map(|(id, arc)| (id.deref().to_owned(), arc))
                .collect(),
            paste: None,
        }
    }

    fn draw_grid(
        pos: Vec2f,
        cell_size: Vec2f,
        mid_lines: Vector<2, u32>,
        rect: emath::Rect,
        paint: &egui::Painter,
    ) {
        let pos = pos * cell_size;
        let visible_cells = (Vec2f::from(rect.size()) / cell_size).convert(|v| v as i32 + 2);
        let start = (pos / cell_size).convert(|v| v as i32);
        let off = pos % cell_size;

        let dim_stroke = Stroke::new(1.0, Color32::from_gray(64));
        let mid_stroke = Stroke::new(1.5, Color32::from_gray(96));

        for i in 0..visible_cells.x() {
            let x = i + start.x();
            if mid_lines.x() > 0 && x % mid_lines.x() as i32 == 0 {
                continue;
            }

            let pos = rect.left() + cell_size.x() * i as f32 - off.x();
            paint.line_segment(
                [pos2(pos, rect.top()), pos2(pos, rect.bottom())],
                dim_stroke,
            );
        }

        for i in 0..visible_cells.y() {
            let y = i + start.y();
            if mid_lines.y() > 0 && y % mid_lines.y() as i32 == 0 {
                continue;
            }

            let pos = rect.top() + cell_size.y() * i as f32 - off.y();
            paint.line_segment(
                [pos2(rect.left(), pos), pos2(rect.right(), pos)],
                dim_stroke,
            );
        }

        let mid_cells =
            visible_cells.combine_with(mid_lines, |v, m| if m == 0 { 0 } else { v / m as i32 + 2 });
        let mid_off = pos % (cell_size * mid_lines.convert(|v| v as f32));

        for i in 0..mid_cells.x() {
            let pos = rect.left() + cell_size.x() * i as f32 * mid_lines.x() as f32 - mid_off.x();
            paint.line_segment(
                [pos2(pos, rect.top()), pos2(pos, rect.bottom())],
                mid_stroke,
            );
        }

        for i in 0..mid_cells.y() {
            let pos = rect.top() + cell_size.y() * i as f32 * mid_lines.y() as f32 - mid_off.y();
            paint.line_segment(
                [pos2(rect.left(), pos), pos2(rect.right(), pos)],
                mid_stroke,
            );
        }

        if start.x() <= 0 && visible_cells.x() + start.x() >= 0 {
            let pos = rect.left() + cell_size.x() * -start.x() as f32 - off.x();
            paint.line_segment(
                [pos2(pos, rect.top()), pos2(pos, rect.bottom())],
                Stroke::new(1.0, Color32::GREEN),
            );
        }

        if start.y() <= 0 && visible_cells.y() + start.y() >= 0 {
            let pos = rect.top() + cell_size.y() * -start.y() as f32 - off.y();
            paint.line_segment(
                [pos2(rect.left(), pos), pos2(rect.right(), pos)],
                Stroke::new(1.0, Color32::RED),
            );
        }
    }

    fn draw_cross(&mut self, bounds: Rect, paint: &egui::Painter) {
        let mut cross_pos = self
            .pan_zoom
            .to_screen(bounds.left_top().into())
            .world_to_screen(0.0.into());

        *cross_pos.x_mut() = cross_pos.x().clamp(bounds.left(), bounds.right());
        *cross_pos.y_mut() = cross_pos.y().clamp(bounds.top(), bounds.bottom());

        let unit = Vec2f::single_value(self.pan_zoom.scale);

        let cross_stroke = Stroke::new(2.0, Color32::WHITE);

        paint.line_segment(
            [
                pos2(cross_pos.x() - unit.x(), cross_pos.y()),
                pos2(cross_pos.x() + unit.x(), cross_pos.y()),
            ],
            cross_stroke,
        );
        paint.line_segment(
            [
                pos2(cross_pos.x(), cross_pos.y() - unit.y()),
                pos2(cross_pos.x(), cross_pos.y() + unit.y()),
            ],
            cross_stroke,
        );
    }

    fn calc_draw_bounds(&self, rect: Rect) -> TileDrawBounds {
        let screen = &self.pan_zoom;
        let chunk_size: Vec2f = (screen.scale * 16.0).into();

        let screen_tl = screen.pos * screen.scale;
        let screen_br = screen_tl + rect.size();

        TileDrawBounds {
            screen_tl,
            screen_br,

            tiles_tl: (screen_tl / screen.scale).convert(|v| v.floor() as i32),
            tiles_br: (screen_br / screen.scale).convert(|v| v.floor() as i32),

            chunks_tl: (screen_tl / chunk_size).convert(|v| v.floor() as i32),
            chunks_br: (screen_br / chunk_size).convert(|v| v.floor() as i32),
        }
    }

    fn main_update(&mut self, ui: &mut Ui, ctx: &Context) {
        let start_time = Instant::now();

        let rect = ui.max_rect();
        self.pan_zoom.update(ui, rect, self.selected_id.is_none());
        let paint = ui.painter_at(rect);
        let font_id = TextStyle::Monospace.resolve(ui.style());
        let mut grid_ds_cell_size = self.pan_zoom.scale;
        while grid_ds_cell_size < 6.0 {
            grid_ds_cell_size *= 16.0;
        }
        App::draw_grid(
            self.pan_zoom.pos * self.pan_zoom.scale / grid_ds_cell_size,
            grid_ds_cell_size.into(),
            16.into(),
            rect,
            &paint,
        );
        self.draw_cross(rect, &paint);
        let bounds = self.calc_draw_bounds(rect);
        let ctx = PaintContext {
            screen: self.pan_zoom.to_screen(rect.left_top().into()),
            paint: &paint,
            rect,
            bounds,
            ui,
            egui_ctx: ctx,
        };

        let selected_item = match self.selected_id.as_deref() {
            None => SelectedBoardItem::None,
            Some("paste") => match &self.paste {
                Some(p) => SelectedBoardItem::Paste(p),
                None => SelectedBoardItem::None,
            },
            Some("selection") => SelectedBoardItem::Selection,
            Some("wire") => SelectedBoardItem::Wire,
            Some(circ) => match self.circuit_previews.get(circ) {
                Some(p) => SelectedBoardItem::Circuit(p.as_ref()),
                None => SelectedBoardItem::None,
            },
        };

        if let SelectedBoardItem::Circuit(pre) = selected_item {
            if ctx.egui_ctx.input(|input| input.key_pressed(Key::R)) {
                pre.props
                    .write(|p: &mut circuits::props::DirectionProp| p.0 = p.0.rotate_clockwise());
            }
        }

        self.board.update(&ctx, selected_item, self.debug);

        let update_time = Instant::now() - start_time;

        paint.text(
            rect.left_top() + vec2(10.0, 80.0),
            Align2::LEFT_TOP,
            format!(
                r#"Pos: {}
Tile draw bounds: {} - {}
Chunk draw bounds: {} - {}
Time: {:.2} ms
Selected: {:?}

[F9] Debug: {}
[F8] Board reload
[F4] State reset
[R] Rotate

Wire parts drawn: {}
Pressed keys: {:?}
"#,
                self.pan_zoom.pos,
                bounds.tiles_tl,
                bounds.tiles_br,
                bounds.chunks_tl,
                bounds.chunks_br,
                update_time.as_secs_f64() * 1000.0,
                self.selected_id,
                self.debug,
                self.board
                    .wires_drawn
                    .load(std::sync::atomic::Ordering::Relaxed),
                ui.input(|input| input.keys_down.iter().cloned().collect::<Vec<_>>())
            ),
            font_id,
            Color32::WHITE,
        );
    }
}
