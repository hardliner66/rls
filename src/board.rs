use std::{
    collections::{HashMap, HashSet},
    f32::consts::TAU,
    num::NonZeroU32,
    sync::{Arc, RwLock},
};

use eframe::{
    egui::{self, Sense},
    epaint::{Color32, Rounding, Stroke},
};
use emath::{vec2, Rect};

use crate::{
    circuits::{Circuit, CircuitNode, CircuitPin, CircuitPreview, CircuitStateContext},
    containers::{Chunks2D, ChunksLookaround, FixedVec},
    state::{State, StateCollection},
    vector::{IsZero, Vec2f, Vec2i},
    wires::{FoundWireNode, TileWires, Wire, WireDirection, WireNode, WirePart, WirePoint},
    PaintContext,
};

pub struct CircuitBoard {
    pub wires: FixedVec<Wire>,
    pub circuits: FixedVec<Circuit>,
    pub states: StateCollection,

    pub updates: HashSet<usize>,
}

impl CircuitBoard {
    pub fn new() -> Self {
        Self {
            wires: vec![].into(),
            circuits: vec![].into(),
            states: StateCollection::new(),
            updates: Default::default(),
        }
    }

    pub fn create_circuit(&mut self, pos: Vec2i, preview: &dyn CircuitPreview) -> usize {
        let id = self.circuits.first_free_pos();
        let circ = Circuit::create(id, pos, preview);
        self.circuits.set(circ, id);
        id
    }

    pub fn create_wire(&mut self) -> usize {
        let id = self.wires.first_free_pos();
        let wire = Wire {
            id,
            points: HashMap::default(),
        };
        self.wires.set(wire, id);
        id
    }

    pub fn merge_wires(&mut self, id: usize, with: usize, update_states: bool) {
        if !self.wires.exists(id) {
            return;
        }

        let with = match self.wires.remove(with) {
            Some(w) => w,
            None => return,
        };

        let Wire { id: _, points } = with;

        for point in points.values() {
            if let Some(pin) = &point.pin {
                pin.write().unwrap().set_wire(None, Some(id));
            }
        }

        if update_states {
            self.states.update_wire(self.wires.get(id).unwrap());
        }
    }

    pub fn split_wire(
        &mut self,
        id: usize,
        points: &HashSet<Vec2i>,
        update_states: bool,
    ) -> Option<usize> {
        fn search_wire_point(
            nodes: impl Iterator<Item = Vec2i>,
            from: Vec2i,
            vertical: bool,
            inverse: bool,
        ) -> Option<Vec2i> {
            let f_eq_coord = if vertical { from.x() } else { from.y() };
            let f_diff_coord = if vertical { from.y() } else { from.x() };

            nodes
                .filter_map(|p| {
                    let p_eq_coord = if vertical { p.x() } else { p.y() };
                    if p_eq_coord != f_eq_coord {
                        None
                    } else {
                        let p_diff_coord = if vertical { p.y() } else { p.x() };

                        let dist = if inverse {
                            f_diff_coord - p_diff_coord
                        } else {
                            p_diff_coord - f_diff_coord
                        };

                        if dist <= 0 {
                            None
                        } else {
                            Some((p, dist))
                        }
                    }
                })
                .min_by(|a, b| a.1.cmp(&b.1))
                .map(|p| p.0)
        }

        let new_wire_id = self.wires.first_free_pos();
        let wire = match self.wires.get_mut(id) {
            None => return None,
            Some(w) => w,
        };
        
        let mut new_points = HashMap::new();
        for pos in points.iter() {
            let point = match wire.points.remove(pos) {
                None => continue,
                Some(p) => p,
            };

            let ref_right = search_wire_point(wire.points.keys().cloned(), *pos, false, true);
            let ref_down = search_wire_point(wire.points.keys().cloned(), *pos, true, true);
            if let Some(r) = ref_right {
                if let Some(p) = wire.points.get_mut(&r) {
                    p.left = false
                }
            }
            if let Some(r) = ref_down {
                if let Some(p) = wire.points.get_mut(&r) {
                    p.up = false
                }
            }

            let WirePoint { left, up, pin } = point;

            let left = left
                && search_wire_point(wire.points.keys().cloned(), *pos, false, false)
                    .is_some_and(|p| points.contains(&p));
            let up = up
                && search_wire_point(wire.points.keys().cloned(), *pos, true, false)
                    .is_some_and(|p| points.contains(&p));
            let pin = pin.map(|p| {
                p.write().unwrap().set_wire(None, Some(new_wire_id));
                p
            });

            new_points.insert(*pos, WirePoint { up, left, pin });
        }

        let new_wire = Wire {
            id: new_wire_id,
            points: new_points,
        };
        self.wires.set(new_wire, new_wire_id);

        if update_states {
            self.states.update_wire(self.wires.get(id).unwrap());
            self.states
                .update_wire(self.wires.get(new_wire_id).unwrap());
        }

        Some(new_wire_id)
    }
}

pub enum SelectedBoardItem<'a> {
    None,
    Wire,
    Circuit(&'a dyn CircuitPreview),
}

impl<'a> SelectedBoardItem<'a> {
    pub fn none(&self) -> bool {
        matches!(self, SelectedBoardItem::None)
    }

    pub fn wire(&self) -> bool {
        matches!(self, SelectedBoardItem::Wire)
    }

    pub fn circuit(&self) -> Option<&'a dyn CircuitPreview> {
        match self {
            SelectedBoardItem::Circuit(c) => Some(*c),
            _ => None,
        }
    }
}

pub struct ActiveCircuitBoard {
    pub board: Arc<RwLock<CircuitBoard>>,
    pub state: Arc<State>,

    wire_nodes: Chunks2D<16, WireNode>,
    circuit_nodes: Chunks2D<16, CircuitNode>,

    wire_drag_pos: Option<Vec2i>,
}

impl ActiveCircuitBoard {
    pub fn new(board: Arc<RwLock<CircuitBoard>>, state: usize) -> Option<Self> {
        let state = {
            let board = board.read().unwrap();
            board.states.get(state)?
        };

        Some(Self {
            board,
            wire_nodes: Default::default(), // TODO
            circuit_nodes: Default::default(),
            state,
            wire_drag_pos: None,
        })
    }

    pub fn update(&mut self, ctx: &PaintContext, selected: SelectedBoardItem) {
        ctx.draw_chunks(
            &self.wire_nodes,
            &self,
            |node| !node.is_empty(),
            |node, pos, ctx, this, lookaround| {
                this.draw_wire_node(ctx, node, pos, lookaround);
            },
        );

        ctx.draw_chunks(
            &self.circuit_nodes,
            &*self,
            |n| n.circuit.is_some(),
            |node, pos, ctx, this, _| this.draw_circuit_node(node, pos, ctx),
        );

        self.update_wires(ctx, selected.wire());
        self.update_circuits(ctx, selected.circuit());
    }

    pub fn update_circuits(&mut self, ctx: &PaintContext, selected: Option<&dyn CircuitPreview>) {
        if let Some(p) = selected {
            let mouse_tile_pos = ctx
                .egui_ctx
                .input(|input| input.pointer.interact_pos())
                .map(|p| ctx.screen.screen_to_world(Vec2f::from(p)));
            let mouse_tile_pos_i = match mouse_tile_pos {
                None => return,
                Some(v) => v.convert_values(|v| v.floor() as i32),
            };
            let size = p.size();
            if size.x() == 0 || size.y() == 0 {
                return;
            }
            let place_pos = mouse_tile_pos_i - size.convert_values(|v| v as i32) / 2;
            let rect = Rect::from_min_size(
                ctx.screen.world_to_screen_tile(place_pos).into(),
                (size.convert_values(|v| v as f32) * ctx.screen.scale).into(),
            );
            p.draw_preview(&ctx.with_rect(rect));
            let interaction = ctx.ui.interact(ctx.rect, ctx.ui.id(), Sense::click());

            if interaction.clicked_by(eframe::egui::PointerButton::Primary) {
                self.place_circuit(place_pos, p);
            }
        }
    }

    fn draw_circuit_node(&self, node: &CircuitNode, pos: Vec2i, ctx: &PaintContext) {
        if !node.origin_dist.is_zero()
            && pos.x() != ctx.bounds.tiles_tl.x()
            && pos.y() != ctx.bounds.tiles_tl.y()
        {
            return;
        }
        let circ_id = match node.circuit.get() {
            None => return,
            Some(c) => c,
        };

        let board = self.board.read().unwrap();
        let circuit = match board.circuits.get(circ_id) {
            None => return,
            Some(c) => c,
        };

        let circ_info = circuit.info.read().unwrap();

        let pos = pos - node.origin_dist.convert_values(|v| v as i32);
        let screen_pos = ctx.screen.world_to_screen_tile(pos);
        let screen_size = circ_info.size.convert_values(|v| v as f32) * ctx.screen.scale;
        let rect = Rect::from_min_size(screen_pos.into(), screen_size.into());
        let circ_ctx = ctx.with_rect(rect);

        let state_ctx = CircuitStateContext::new(&self.state, circuit);

        circuit.imp.read().unwrap().draw(&state_ctx, &circ_ctx);
    }

    fn place_circuit(&mut self, place_pos: Vec2i, preview: &dyn CircuitPreview) {
        let size = preview.size();

        for j in 0..size.y() {
            for i in 0..size.x() {
                let x = place_pos.x() + i as i32;
                let y = place_pos.y() + j as i32;
                if self
                    .circuit_nodes
                    .get(x as isize, y as isize)
                    .is_some_and(|n| n.circuit.is_some())
                {
                    return;
                }
            }
        }

        let cid = {
            self.board
                .write()
                .unwrap()
                .create_circuit(place_pos, preview)
        };

        for j in 0..size.y() {
            for i in 0..size.x() {
                let x = place_pos.x() + i as i32;
                let y = place_pos.y() + j as i32;
                let node = self.circuit_nodes.get_or_create_mut(x as isize, y as isize);

                node.circuit.set(Some(cid));
                node.origin_dist = [i, j].into();
            }
        }
        let circ_info = { self.board.read().unwrap().circuits.get(cid).unwrap().info.clone() };
        for pin in circ_info.read().unwrap().pins.iter() {
            let pos = place_pos + pin.pos.convert_values(|v| v as i32);
            if let Some(wire) = self.create_wire_intersection(pos, None) {
                let wire = if let Some(wire) = self.board.write().unwrap().wires.get_mut(wire) {
                    if let Some(p) = wire.points.get_mut(&pos) {
                        p.pin = Some(pin.pin.clone());
                    }
                    Some(wire.id)
                } else {
                    None
                };

                if let Some(wire) = wire {
                    pin.pin
                        .write()
                        .unwrap()
                        .set_wire(Some(&self.board.read().unwrap()), Some(wire));
                }
            }
        }
        let board = self.board.read().unwrap();
        let circ = board.circuits.get(cid).unwrap();
        self.board
            .read()
            .unwrap()
            .states
            .update_circuit_signals(circ, None);

        let state_ctx = CircuitStateContext::new(&self.state, circ);
        let update_interval = circ.imp.read().unwrap().update_interval(&state_ctx);

        drop(board);

        if let Some(duration) = update_interval {
            self.board.write().unwrap().updates.insert(cid);
            self.board
                .read()
                .unwrap()
                .states
                .update_circuit_interval(cid, duration);
        }
    }

    pub fn pin_at(&self, pos: Vec2i) -> Option<Arc<RwLock<CircuitPin>>> {
        let circ = self.circuit_nodes.get(pos.x() as isize, pos.y() as isize)?;
        let pos = circ.origin_dist;
        let circ = circ.circuit.get()?;

        let board = self.board.read().unwrap();
        let circ_info = board.circuits.get(circ)?.info.clone();
        let circ_info = circ_info.read().unwrap();

        circ_info
            .pins
            .iter()
            .find(|p| p.pos == pos)
            .map(|p| p.pin.clone())
    }

    fn update_wires(&mut self, ctx: &PaintContext, selected: bool) {

        if !selected {
            self.wire_drag_pos = None;
            return;
        }

        let mouse_tile_pos = ctx
            .egui_ctx
            .input(|input| input.pointer.interact_pos())
            .map(|p| ctx.screen.screen_to_world(Vec2f::from(p)));

        let mouse_tile_pos_i = mouse_tile_pos.map(|p| p.convert_values(|v| v.floor() as i32));

        let drawing_wire = Self::calc_wire_part(self.wire_drag_pos, mouse_tile_pos_i);
        if let Some(ref part) = drawing_wire {
            self.draw_wire_part(ctx, part, Color32::GRAY);
        }

        let interaction = ctx
            .ui
            .interact(ctx.rect, ctx.ui.id(), Sense::click_and_drag());

        if self.wire_drag_pos.is_none() && interaction.drag_started_by(egui::PointerButton::Primary)
        {
            self.wire_drag_pos = mouse_tile_pos_i;
        } else if self.wire_drag_pos.is_some()
            && interaction.drag_released_by(egui::PointerButton::Primary)
        {
            self.wire_drag_pos = None;

            if let Some(part) = drawing_wire {
                self.place_wire_part(part);
            }
        }

        if let Some(mouse_pos) = mouse_tile_pos_i {
            if interaction.clicked_by(egui::PointerButton::Primary) && self.wire_drag_pos.is_none()
            {
                self.try_toggle_node_intersection(mouse_pos);
            }

            for i in 0..4 {
                let vertical = i & 1 == 1;
                let forward = i & 2 == 2;
                if let Some(found) = self.find_node(mouse_pos, vertical, forward) {
                    let world_pos = ctx.screen.world_to_screen_tile(found.pos);
                    let size = vec2(ctx.screen.scale, ctx.screen.scale);
                    let rect = Rect::from_min_size(world_pos.into(), size);

                    let r = if vertical { 255 } else { 0 };
                    let g = if forward { 255 } else { 0 };
                    let color = Color32::from_rgba_unmultiplied(r, g, 0, 128);
                    ctx.paint.rect_filled(rect, Rounding::none(), color);
                }
            }
        }
    }

    fn try_toggle_node_intersection(&mut self, pos: Vec2i) {
        let node = self.wire_nodes.get(pos.x() as isize, pos.y() as isize);

        let node = match node {
            None => return,
            Some(n) if n.is_empty() => return,
            Some(v) => v,
        };

        let center = node.wire.is_some();
        let left = node.left > 0;
        let up = node.up > 0;
        let right = (center || left)
            && self
                .wire_nodes
                .get(pos.x() as isize + 1, pos.y() as isize)
                .is_some_and(|n| {
                    let pointer = if center { 1 } else { node.left + 1 };
                    n.left == pointer
                });
        let down = (center || up)
            && self
                .wire_nodes
                .get(pos.x() as isize, pos.y() as isize + 1)
                .is_some_and(|n| {
                    let pointer = if center { 1 } else { node.up + 1 };
                    n.up == pointer
                });

        if up != down || left != right {
            return;
        }

        if center {
            self.remove_intersection_at_node(pos, *node, true);
        } else {
            self.create_wire_intersection_at_node(pos, *node, None);
        }
    }

    fn draw_wire_node(
        &self,
        ctx: &PaintContext<'_>,
        node: &WireNode,
        pos: Vec2i,
        lookaround: &ChunksLookaround<'_, 16, WireNode>,
    ) {
        struct WireDrawInfo {
            dist: u32,
            next_dist: u32,
            wire: Option<usize>,
            vertical: bool,
            pos: Vec2i,
            color: Color32,
        }

        fn draw_wire(info: WireDrawInfo, this: &ActiveCircuitBoard, ctx: &PaintContext) {
            if info.dist == 0 && info.wire.is_none() {
                return;
            }

            let edge = match info.vertical {
                true => info.pos.y() == ctx.bounds.tiles_br.y(),
                false => info.pos.x() == ctx.bounds.tiles_br.x(),
            };

            if (info.wire.is_none() || info.dist == 0) && !edge {
                return;
            }

            let length = match info.next_dist {
                0 => {
                    if info.dist == 0 {
                        return;
                    } else {
                        info.dist
                    }
                }
                node_dist => {
                    if node_dist == info.dist + 1 {
                        node_dist
                    } else {
                        info.dist
                    }
                }
            };

            let part = WirePart {
                length: NonZeroU32::new(length).unwrap(),
                pos: info.pos
                    - if info.vertical {
                        [0, info.dist as i32]
                    } else {
                        [info.dist as i32, 0]
                    },
                vertical: info.vertical,
            };

            this.draw_wire_part(ctx, &part, info.color)
        }
        if node.wire.is_none() && node.up == 0 && node.left == 0 {
            return;
        }
        let wires = self.wires_at_node(pos, node);
        let wire_color_v = wires.up().map_or(Color32::from_rgb(255, 100, 255), |w| {
            self.state.read_wire(w).color()
        });
        let wire_color_h = wires.left().map_or(Color32::from_rgb(255, 100, 255), |w| {
            self.state.read_wire(w).color()
        });
        let next_node_v = lookaround.get_relative(0, 1);
        let next_node_h = lookaround.get_relative(1, 0);

        let draw_h = WireDrawInfo {
            dist: node.left,
            next_dist: next_node_h.map_or(0, |n| n.left),
            wire: node.wire.get(),
            vertical: false,
            pos,
            color: wire_color_h,
        };

        let draw_v = WireDrawInfo {
            dist: node.up,
            next_dist: next_node_v.map_or(0, |n| n.up),
            wire: node.wire.get(),
            vertical: true,
            pos,
            color: wire_color_v,
        };

        draw_wire(draw_h, self, ctx);
        draw_wire(draw_v, self, ctx);
        if let Some(wire) = node.wire.get() {
            let possible_intersection = if ctx.egui_ctx.input(|input| input.modifiers.shift) {
                true
            } else {
                node.left > 0
                    && node.up > 0
                    && next_node_h.is_some_and(|n| n.left == 1)
                    && next_node_v.is_some_and(|n| n.up == 1)
            };

            if possible_intersection {
                if let Some(wire) = self.board.read().unwrap().wires.get(wire) {
                    Self::draw_wire_intersection(
                        ctx,
                        pos,
                        wire.color(&self.state),
                        wire.points.get(&pos).is_some_and(|p| p.pin.is_some()),
                    )
                }
            }
        }
        let correct_up = node.up == 0
            || lookaround
                .get_relative(0, -(node.up as isize))
                .is_some_and(|n| n.wire.is_some())
                && (1..node.up as isize).all(|p| {
                    lookaround
                        .get_relative(0, -p)
                        .is_some_and(|n| n.wire.is_none())
                });
        let correct_left = node.left == 0
            || lookaround
                .get_relative(-(node.left as isize), 0)
                .is_some_and(|n| n.wire.is_some())
                && (1..node.left as isize).all(|p| {
                    lookaround
                        .get_relative(-p, 0)
                        .is_some_and(|n| n.wire.is_none())
                });
        if !correct_up || !correct_left {
            let pos = ctx.screen.world_to_screen_tile(pos);

            let rect = Rect::from_min_size(pos.into(), vec2(ctx.screen.scale, ctx.screen.scale));
            ctx.paint.rect_filled(
                rect,
                Rounding::none(),
                Color32::from_rgba_unmultiplied(255, 0, 0, 100),
            );
        }
    }

    fn draw_wire_part(&self, ctx: &PaintContext, part: &WirePart, color: Color32) {
        let screen = &ctx.screen;
        let thickness = ctx.screen.scale * 0.25;

        let pos = screen.world_to_screen_tile(part.pos) + ((screen.scale - thickness) * 0.5);
        let length = screen.scale * part.length.get() as f32 + thickness;

        let rect_size = match part.vertical {
            true => vec2(thickness, length),
            false => vec2(length, thickness),
        };
        let rect = Rect::from_min_size(pos.into(), rect_size);
        ctx.paint.rect_filled(rect, Rounding::none(), color);
    }

    fn draw_wire_intersection(ctx: &PaintContext, pos: Vec2i, color: Color32, pin: bool) {
        let screen = &ctx.screen;
        let thickness = screen.scale * 0.4;

        let pos = screen.world_to_screen_tile(pos) + ((screen.scale - thickness) * 0.5);

        let rect = Rect::from_min_size(pos.into(), vec2(thickness, thickness));
        ctx.paint.rect_filled(rect, Rounding::none(), color);

        // DEBUG: visuals
        if pin {
            ctx.paint
                .rect_stroke(rect, Rounding::none(), Stroke::new(1.0, Color32::RED));
        }
    }

    fn calc_wire_part(from: Option<Vec2i>, to: Option<Vec2i>) -> Option<WirePart> {
        if let Some(from) = from {
            if let Some(to) = to {
                if from != to {
                    let angle = (to - from).convert_values(|v| v as f32).angle_to_x();
                    let axis = (angle / (TAU / 4.0)).round() as i32 % 4;

                    let origin = match axis {
                        1 => [from.x(), to.y()].into(),
                        2 => [to.x(), from.y()].into(),
                        _ => from,
                    };
                    let length = match axis {
                        0 => to.x() - from.x(),
                        1 => from.y() - to.y(),
                        2 => from.x() - to.x(),
                        3 => to.y() - from.y(),
                        _ => unreachable!(),
                    } as u32;

                    let vertical = matches!(axis, 1 | 3);

                    let part = WirePart {
                        pos: origin,
                        length: NonZeroU32::new(length).unwrap(),
                        vertical,
                    };

                    Some(part)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn create_wire_intersection(&mut self, pos: Vec2i, wire: Option<usize>) -> Option<usize> {
        let node = self.wire_nodes.get(pos.x() as isize, pos.y() as isize);
        let node = match node {
            Some(v) => *v,
            None => return None,
        };
        self.create_wire_intersection_at_node(pos, node, wire)
    }

    // wire param: if Some(w) will use wire w, if None, will try to figure ot which wire to use (with merging wires)
    pub fn create_wire_intersection_at_node(
        &mut self,
        pos: Vec2i,
        node: WireNode,
        wire: Option<usize>,
    ) -> Option<usize> {
        if node.wire.is_some() || node.up == 0 && node.left == 0 {
            return node.wire.get();
        }

        let wire_up = self
            .find_node_from_node(&node, pos, true, false)
            .map(|f| f.wire);
        let wire_left = self
            .find_node_from_node(&node, pos, false, false)
            .map(|f| f.wire);

        let wire = wire.or_else(|| match (wire_up, wire_left) {
            (None, None) => None,
            (None, Some(l)) => Some(l),
            (Some(u), None) => Some(u),
            (Some(u), Some(l)) => {
                let (left_nodes, up_nodes) = {
                    let board = self.board.read().unwrap();
                    (
                        board.wires.get(l).map(|w| w.points.len()),
                        board.wires.get(u).map(|w| w.points.len()),
                    )
                };

                match (left_nodes, up_nodes) {
                    (Some(un), Some(ln)) => {
                        if un > ln {
                            self.merge_wires(u, l, false);
                            Some(u)
                        } else {
                            self.merge_wires(l, u, false);
                            Some(l)
                        }
                    }
                    _ => None,
                }
            }
        });
        let wire = match wire {
            Some(w) => w,
            None => return None,
        };

        self.set_wire_point(pos, Some(wire), true);
        Some(wire)
    }

    #[allow(unused)]
    fn remove_intersection(&mut self, pos: Vec2i, split: bool) {
        if let Some(node) = self.wire_nodes.get(pos.x() as isize, pos.y() as isize) {
            self.remove_intersection_at_node(pos, *node, split)
        }
    }

    // split param: if intersection was of 2 wires, split them
    fn remove_intersection_at_node(&mut self, pos: Vec2i, node: WireNode, split: bool) {
        if node.wire.is_none()
            || node.up == 0 && node.left == 0
            || node.wire.get().is_some_and(|w| {
                self.board
                    .read()
                    .unwrap()
                    .wires
                    .get(w)
                    .is_some_and(|w| w.points.get(&pos).is_some_and(|p| p.pin.is_some()))
            })
        {
            return;
        }

        let split = split && node.up > 0 && node.left > 0;

        let prev = self.set_wire_point(pos, None, !split);

        if split {
            if let Some(wire) = prev {
                self.split_wires(wire, true);
            }
        }
    }

    fn place_wire_part(&mut self, part: WirePart) {
        let part = match self.optimize_wire_part(part) {
            Some(p) => p,
            None => return,
        };

        let pins_crossed = {
            let mut pins_crossed = HashMap::new();
            for i in 0..=part.length.get() {
                let pos = part.pos
                    + match part.vertical {
                        true => [0, i as i32],
                        false => [i as i32, 0],
                    };

                let pin = match self.pin_at(pos) {
                    Some(v) => v,
                    None => continue,
                };

                pins_crossed.insert(pos, pin);
            }
            pins_crossed
        };

        let wires_crossed = {
            let mut wires_crossed = HashSet::new();
            for i in 0..=part.length.get() {
                let pos = part.pos
                    + match part.vertical {
                        true => [0, i as i32],
                        false => [i as i32, 0],
                    };

                let node = match self.wire_nodes.get(pos.x() as isize, pos.y() as isize) {
                    None => continue,
                    Some(v) => v,
                };

                if i == 0 || i == part.length.get() {
                    match self.wires_at(pos) {
                        TileWires::None => (),
                        TileWires::One { wire, dir: _ } => {
                            wires_crossed.insert(wire);
                        }
                        TileWires::Two { left, up } => {
                            wires_crossed.insert(left);
                            wires_crossed.insert(up);
                        }
                    }
                } else if let Some(wire) = node.wire.get() {
                    wires_crossed.insert(wire);
                }
            }
            wires_crossed
        };

        let new_wire = match wires_crossed.len() {
            0 => self.board.write().unwrap().create_wire(),
            1 => *wires_crossed.iter().next().unwrap(),
            _ => {
                let main_wire = {
                    self.board
                        .read()
                        .unwrap()
                        .wires
                        .iter()
                        .filter(|v| wires_crossed.contains(&v.id))
                        .max_by(|x, y| x.points.len().cmp(&y.points.len()))
                        .expect("Some matching wires")
                        .id
                };

                for wire in wires_crossed.iter() {
                    if *wire != main_wire {
                        self.merge_wires(main_wire, *wire, false)
                    }
                }
                main_wire
            }
        };

        self.set_wire_point(part.pos, Some(new_wire), false);
        self.set_wire_point(
            part.pos
                + match part.vertical {
                    true => [0, part.length.get() as i32],
                    false => [part.length.get() as i32, 0],
                },
            Some(new_wire),
            false,
        );

        let mut dist = 0;

        for i in 0..=part.length.get() {
            let pos = part.pos
                + match part.vertical {
                    true => [0, i as i32],
                    false => [i as i32, 0],
                };

            let node = self
                .wire_nodes
                .get_or_create_mut(pos.x() as isize, pos.y() as isize);

            if i > 0 {
                match part.vertical {
                    true => node.up = dist,
                    false => node.left = dist,
                }
            }

            let crossed_pin = pins_crossed.get(&pos).is_some();
            let crossed_wire = node.wire.is_some();
            let point = crossed_pin || crossed_wire;

            if point {
                dist = 1
            } else {
                dist += 1;
            }
            if i == 0 || i == part.length.get() || point {
                node.wire.set(Some(new_wire));
                self.board
                    .write()
                    .unwrap()
                    .wires
                    .get_mut(new_wire)
                    .expect("unreachable")
                    .add_point(
                        pos,
                        None,
                        node.left > 0,
                        node.up > 0,
                        pins_crossed.get(&pos).cloned(),
                    )
            }
        }

        let board = self.board.read().unwrap();
        board
            .states
            .update_wire(self.board.read().unwrap().wires.get(new_wire).unwrap());
    }

    fn optimize_wire_part(&mut self, part: WirePart) -> Option<WirePart> {
        let mut part_pos = part.pos;
        let mut part_len = part.length.get() as i32;

        let new_start = self
            .wire_nodes
            .get(part_pos.x() as isize, part_pos.y() as isize)
            .and_then(|n| {
                if n.wire.is_some() {
                    None
                } else {
                    self.find_node_from_node(n, part_pos, part.vertical, true)
                }
            });

        if let Some(found) = new_start {
            part_len -= found.distance.get() as i32;
            part_pos = found.pos;
        }

        let end_pos = match part.vertical {
            true => part_pos + [0, part_len],
            false => part_pos + [part_len, 0],
        };

        let new_end_dist = self
            .wire_nodes
            .get(end_pos.x() as isize, end_pos.y() as isize)
            .and_then(|n| {
                self.find_node_from_node(n, end_pos, part.vertical, false)
                    .map(|v| v.distance)
            });

        match new_end_dist {
            None => {}
            Some(dist) => {
                part_len -= dist.get() as i32;
            }
        }

        if part_len <= 0 {
            return None;
        }
        let part = WirePart {
            pos: part_pos,
            length: NonZeroU32::new(part_len as u32).unwrap(),
            vertical: part.vertical,
        };
        Some(part)
    }

    fn wires_at(&self, pos: Vec2i) -> TileWires {
        match self.wire_nodes.get(pos.x() as isize, pos.y() as isize) {
            None => TileWires::None,
            Some(node) => self.wires_at_node(pos, node),
        }
    }

    fn wires_at_node(&self, pos: Vec2i, node: &WireNode) -> TileWires {
        if let Some(wire) = node.wire.get() {
            return TileWires::One {
                wire,
                dir: WireDirection::None,
            };
        }

        let up = match node.up {
            0 => None,
            up => self
                .wire_nodes
                .get(pos.x() as isize, (pos.y() - up as i32) as isize)
                .and_then(|n| n.wire.get()),
        };
        let left = match node.left {
            0 => None,
            left => self
                .wire_nodes
                .get((pos.x() - left as i32) as isize, pos.y() as isize)
                .and_then(|n| n.wire.get()),
        };

        match (left, up) {
            (None, None) => TileWires::None,
            (None, Some(u)) => TileWires::One {
                wire: u,
                dir: WireDirection::Up,
            },
            (Some(l), None) => TileWires::One {
                wire: l,
                dir: WireDirection::Left,
            },
            (Some(left), Some(up)) => TileWires::Two { left, up },
        }
    }

    fn merge_wires(&mut self, wire: usize, with: usize, update_state: bool) {
        {
            let board = self.board.read().unwrap();

            let with_wire = match board.wires.get(with) {
                Some(w) => w,
                None => return,
            };

            if !board.wires.exists(wire) || wire == with {
                return;
            }

            let points: Vec<_> = with_wire.points.keys().cloned().collect();
            drop(board);

            self.set_node_wires(points.iter(), wire);
        }
        self.board
            .write()
            .unwrap()
            .merge_wires(wire, with, update_state);
    }

    fn find_node(&self, pos: Vec2i, vertical: bool, forward: bool) -> Option<FoundWireNode> {
        let node = self.wire_nodes.get(pos.x() as isize, pos.y() as isize)?;
        self.find_node_from_node(node, pos, vertical, forward)
    }

    fn find_node_from_node(
        &self,
        node: &WireNode,
        pos: Vec2i,
        vertical: bool,
        forward: bool,
    ) -> Option<FoundWireNode> {
        let pointer = match vertical {
            true => node.up,
            false => node.left,
        };

        if forward {
            let start = if node.wire.is_some() { 0 } else { pointer };
            for i in 1.. {
                let offset = start + i;
                let target_pos = match vertical {
                    true => pos + [0, i as i32],
                    false => pos + [i as i32, 0],
                };
                let target = self
                    .wire_nodes
                    .get(target_pos.x() as isize, target_pos.y() as isize)?;
                let target_pointer = match vertical {
                    true => target.up,
                    false => target.left,
                };
                if target_pointer != offset {
                    break;
                }
                if let Some(wire) = target.wire.get() {
                    return Some(FoundWireNode {
                        node: *target,
                        wire,
                        pos: target_pos,
                        distance: NonZeroU32::new(i).unwrap(),
                    });
                }
            }
            None
        } else if pointer == 0 {
            None
        } else {
            let target_pos = match vertical {
                true => pos - [0, pointer as i32],
                false => pos - [pointer as i32, 0],
            };
            let target = self
                .wire_nodes
                .get(target_pos.x() as isize, target_pos.y() as isize)?;
            target.wire.get().map(|w| FoundWireNode {
                node: *target,
                wire: w,
                pos: target_pos,
                distance: NonZeroU32::new(pointer).unwrap(),
            })
        }
    }

    fn split_wires(&mut self, id: usize, update_states: bool) {
        let mut groups = vec![];

        let mut remaining_nodes: HashSet<_> = {
            let board = self.board.read().unwrap();
            let wire = match board.wires.get(id) {
                Some(w) => w,
                None => return,
            };
            wire.points.keys().copied().collect()
        };
        let mut queue = vec![];

        while !remaining_nodes.is_empty() {
            let mut group = HashSet::new();
            let start = *remaining_nodes.iter().next().unwrap();
            queue.push(start);

            while let Some(pos) = queue.pop() {
                if !remaining_nodes.remove(&pos) {
                    continue;
                }

                group.insert(pos);

                let (ints, intc) = self.node_neighboring_intersections(pos, Some(id));
                (0..intc).for_each(|inti| {
                    let int = ints[inti];
                    if remaining_nodes.contains(&int) {
                        queue.push(int);
                    }
                });
            }
            groups.push(group);
        }

        if groups.len() <= 1 {
            return;
        }

        let biggest_wire = groups
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.len().cmp(&b.1.len()))
            .unwrap()
            .0;

        let mut wires = vec![];

        for (groupid, group) in groups.drain(..).enumerate() {
            if groupid != biggest_wire {
                let new_wire = self.board.write().unwrap().split_wire(id, &group, false);
                if let Some(wire) = new_wire {
                    self.set_node_wires(group.iter(), wire);
                    wires.push(wire);
                }
            } else {
                wires.push(id);
            }
        }

        if update_states {
            for wire in wires {
                self.board
                    .read()
                    .unwrap()
                    .states
                    .update_wire(self.board.read().unwrap().wires.get(wire).unwrap());
            }
        }
    }

    fn node_neighboring_intersections(
        &self,
        pos: Vec2i,
        wire: Option<usize>,
    ) -> ([Vec2i; 4], usize) {
        let mut arr = [Vec2i::default(); 4];
        let mut arrpos = 0;

        let node = match self.wire_nodes.get(pos.x() as isize, pos.y() as isize) {
            None => return (arr, 0),
            Some(v) => v,
        };

        for i in 0..4 {
            let vertical = i & 1 == 1;
            let forward = i & 2 == 2;
            if let Some(f) = self.find_node_from_node(node, pos, vertical, forward) {
                if !wire.is_some_and(|v| v != f.wire) {
                    arr[arrpos] = f.pos;
                    arrpos += 1;
                }
            }
        }
        (arr, arrpos)
    }

    fn set_node_wires<'a>(&mut self, positions: impl Iterator<Item = &'a Vec2i>, wire: usize) {
        for npos in positions {
            match self
                .wire_nodes
                .get_mut(npos.x() as isize, npos.y() as isize)
            {
                None => {}
                Some(n) => n.wire.set(Some(wire)),
            }
        }
    }

    /// returns previous wire id
    fn set_wire_point(
        &mut self,
        pos: Vec2i,
        wire: Option<usize>,
        update_state: bool,
    ) -> Option<usize> {
        fn fix_pointers(
            wires: &mut Chunks2D<16, WireNode>,
            pos: Vec2i,
            vertical: bool,
            from: u32,
            to: u32,
            possibly_remove: bool,
        ) {
            let increment_to = to > 0 || !possibly_remove;
            let dir = if vertical { [0, 1] } else { [1, 0] };

            for i in 1.. {
                let pos = pos + dir;

                let node = match wires.get_mut(pos.x() as isize, pos.y() as isize) {
                    None => break,
                    Some(b) => b,
                };

                let node_pointer = match vertical {
                    true => &mut node.up,
                    false => &mut node.left,
                };
                if *node_pointer != from + i {
                    break;
                }

                let to = if increment_to { to + i } else { to };
                *node_pointer = to;

                if node.wire.is_some() {
                    break;
                }
            }
        }

        let node = self
            .wire_nodes
            .get_or_create_mut(pos.x() as isize, pos.y() as isize);
        let prev_wire = node.wire.get();
        if prev_wire == wire {
            return prev_wire;
        }

        let left = node.left;
        let up = node.up;

        node.wire.set(wire);

        if wire.is_some() {
            fix_pointers(&mut self.wire_nodes, pos, false, left, 0, false);
            fix_pointers(&mut self.wire_nodes, pos, true, up, 0, false);
        } else {
            fix_pointers(&mut self.wire_nodes, pos, false, 0, left, true);
            fix_pointers(&mut self.wire_nodes, pos, true, 0, up, true);
        }

        let states_to_update = match update_state {
            true => Some(self.board.read().unwrap().states.clone()),
            false => None,
        };

        let mut board = self.board.write().unwrap();

        if let Some(wire) = prev_wire.and_then(|w| board.wires.get_mut(w)) {
            wire.remove_point(pos, states_to_update.clone());
        }

        if let Some(wire) = wire.and_then(|w| board.wires.get_mut(w)) {
            wire.add_point(pos, states_to_update, left > 0, up > 0, self.pin_at(pos));
        }

        prev_wire
    }
}