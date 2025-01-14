#![allow(incomplete_features)]
#![feature(generic_const_exprs)]
#![feature(int_roundings)]
#![feature(lazy_cell)]
#![feature(thread_id_value)]

use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
    f32::consts::{PI, TAU},
    hash::Hash,
    num::NonZeroU32,
    ops::{Deref, Range},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use board::{selection::Selection, ActiveCircuitBoard};
use cache::GLOBAL_STR_CACHE;
use eframe::{
    egui::{self, Context, Sense, Ui},
    epaint::{Color32, PathShape, Rounding, Shape, Stroke},
};
use emath::{pos2, vec2, Pos2, Rect};

use serde::{Deserialize, Serialize};
#[cfg(feature = "wasm")]
use wasm_bindgen::{prelude::*, JsValue};

mod r#const;

mod vector;

use ui::InventoryItem;
use vector::{Vec2f, Vec2i, Vec2u};
use wires::WirePart;

mod containers;
use crate::containers::*;

mod circuits;
use circuits::CircuitPreview;

mod wires;

mod state;
use state::{State, WireState};

mod board;

#[macro_use]
mod macros;

#[cfg(all(feature = "deadlock_detection", not(feature = "single_thread")))]
mod debug;

mod app;
mod cache;
mod io;
mod path;
mod time;
mod ui;

#[cfg(all(feature = "deadlock_detection", not(feature = "single_thread")))]
type RwLock<T> = debug::DebugRwLock<T>;
#[cfg(all(feature = "deadlock_detection", not(feature = "single_thread")))]
type Mutex<T> = debug::DebugMutex<T>;

#[cfg(any(not(feature = "deadlock_detection"), feature = "single_thread"))]
type RwLock<T> = parking_lot::RwLock<T>;
#[cfg(any(not(feature = "deadlock_detection"), feature = "single_thread"))]
type Mutex<T> = parking_lot::Mutex<T>;

struct BasicLoadingContext<'a, K: Borrow<str> + Eq + Hash> {
    previews: &'a HashMap<K, Arc<CircuitPreview>>,
}

impl<K: Borrow<str> + Eq + Hash> io::LoadingContext for BasicLoadingContext<'_, K> {
    fn get_circuit_preview<'a>(&'a self, ty: &str) -> Option<&'a CircuitPreview> {
        self.previews.get(ty).map(|b| b.deref())
    }
}

fn main() {
    #[cfg(all(feature = "deadlock_detection", not(feature = "single_thread")))]
    debug::set_this_thread_debug_name("egui main thread");

    #[cfg(not(feature = "wasm"))]
    eframe::run_native(
        "rls",
        eframe::NativeOptions::default(),
        Box::new(|cc| Box::new(app::App::create(cc))),
    )
    .unwrap();
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
#[cfg(feature = "wasm")]
pub async fn web_main(canvas_id: &str) -> Result<(), JsValue> {
    use eframe::WebOptions;

    eframe::WebRunner::new()
        .start(
            canvas_id,
            WebOptions::default(),
            Box::new(|cc| Box::new(app::App::create(cc))),
        )
        .await
}

#[allow(unused)]
#[derive(Debug, Clone, Copy)]
struct TileDrawBounds {
    pub screen_tl: Vec2f,
    pub screen_br: Vec2f,

    pub tiles_tl: Vec2i,
    pub tiles_br: Vec2i,

    pub chunks_tl: Vec2i,
    pub chunks_br: Vec2i,
}

impl TileDrawBounds {
    pub const EVERYTHING: TileDrawBounds = TileDrawBounds {
        screen_tl: Vec2f::single_value(f32::NEG_INFINITY),
        screen_br: Vec2f::single_value(f32::INFINITY),
        tiles_tl: Vec2i::single_value(i32::MIN),
        tiles_br: Vec2i::single_value(i32::MAX),
        chunks_tl: Vec2i::single_value(i32::MIN),
        chunks_br: Vec2i::single_value(i32::MAX),
    };
}

#[allow(clippy::redundant_allocation)]
pub struct PaintContext<'a> {
    screen: Screen,
    paint: &'a egui::Painter,
    rect: Rect,
    bounds: TileDrawBounds,
    ui: &'a Ui,
    egui_ctx: &'a Context,
}

impl<'a> PaintContext<'a> {
    pub fn new_on_ui(ui: &'a Ui, rect: Rect, scale: f32) -> Self {
        Self {
            screen: Screen {
                offset: rect.left_top().into(),
                pos: 0.0.into(),
                scale,
            },
            paint: ui.painter(),
            rect,
            bounds: TileDrawBounds::EVERYTHING,
            ui,
            egui_ctx: ui.ctx(),
        }
    }

    pub fn with_rect(&self, rect: Rect) -> PaintContext<'a> {
        Self {
            rect,
            ui: self.ui,
            ..*self
        }
    }

    fn draw_chunks<const CHUNK_SIZE: usize, T: Default, P>(
        &self,
        chunks: &Chunks2D<CHUNK_SIZE, T>,
        pass: &P,
        draw_tester: impl Fn(&T) -> bool,
        drawer: impl Fn(&T, Vec2i, &Self, &P, &ChunksLookaround<CHUNK_SIZE, T>),
    ) {
        let TileDrawBounds {
            screen_tl: _,
            screen_br: _,
            tiles_tl,
            tiles_br,
            chunks_tl,
            chunks_br,
        } = self.bounds;

        let screen = &self.screen;

        for cy in chunks_tl.y()..=chunks_br.y() {
            let rowrange = chunks.get_chunk_row_range(cy as isize);
            let rowrange = Range {
                start: rowrange.start as i32,
                end: rowrange.end as i32,
            };

            for cx in (chunks_tl.x()..chunks_br.x() + 1).intersect(&rowrange) {
                let chunk_coord: Vec2i = [cx, cy].into();
                let chunk_tl = chunk_coord * 16;
                let chunk = unwrap_option_or_continue!(
                    chunks.get_chunk(chunk_coord.convert(|v| v as isize))
                );
                let chunk_viewport_tl = tiles_tl - chunk_tl;
                let chunk_viewport_br = tiles_br - chunk_tl;

                for j in 0..16 {
                    if j < chunk_viewport_tl.y() {
                        continue;
                    } else if j > chunk_viewport_br.y() {
                        break;
                    }

                    for i in 0..16 {
                        if i < chunk_viewport_tl.x() {
                            continue;
                        } else if i > chunk_viewport_br.x() {
                            break;
                        }

                        let tile = &chunk[i as usize][j as usize];
                        if !draw_tester(tile) {
                            continue;
                        }

                        let pos: Vec2i = chunk_tl + [i, j];
                        let draw_pos = Vec2f::from(self.rect.left_top())
                            + screen.world_to_screen(pos.convert(|v| v as f32));
                        let rect =
                            Rect::from_min_size(draw_pos.into(), vec2(screen.scale, screen.scale));
                        let lookaround = ChunksLookaround::new(
                            chunks,
                            chunk,
                            pos.convert(|v| v as isize),
                            [i as usize, j as usize].into(),
                        );

                        let drawer_ctx = self.with_rect(rect);

                        drawer(tile, pos, &drawer_ctx, pass, &lookaround)
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PanAndZoom {
    pos: Vec2f,
    scale: f32,
}

impl PanAndZoom {
    fn update(&mut self, ui: &egui::Ui, rect: Rect, allow_primary_button_drag: bool) {
        let zoom = ui.input(|input| {
            input
                .multi_touch()
                .map(|mt| mt.zoom_delta)
                .unwrap_or_else(|| {
                    let v = input.scroll_delta.y / 240.0;
                    if v < 0.0 {
                        1.0 / (-v + 1.0)
                    } else if v > 0.0 {
                        v + 1.0
                    } else {
                        1.0
                    }
                })
        });

        let interaction = ui.interact(rect, ui.id(), Sense::drag());

        if interaction.dragged_by(egui::PointerButton::Secondary)
            || (allow_primary_button_drag && interaction.dragged_by(egui::PointerButton::Primary))
        {
            self.pos -= interaction.drag_delta() / self.scale;
        }

        if zoom != 1.0 {
            let pointer_screen = Vec2f::from(
                ui.input(|i| i.pointer.hover_pos())
                    .unwrap_or_else(|| rect.center())
                    - rect.left_top(),
            );
            let world_before = self.pos + pointer_screen / self.scale;
            self.scale *= zoom;
            let world_after = self.pos + pointer_screen / self.scale;
            self.pos -= world_after - world_before;
        }
    }
}

impl Default for PanAndZoom {
    fn default() -> Self {
        Self {
            scale: 1.0,
            pos: Default::default(),
        }
    }
}

impl PanAndZoom {
    pub fn new(pos: Vec2f, scale: f32) -> Self {
        Self { pos, scale }
    }

    pub fn to_screen(self, offset: Vec2f) -> Screen {
        Screen {
            offset,
            pos: self.pos,
            scale: self.scale,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Screen {
    offset: Vec2f,
    pos: Vec2f,
    scale: f32,
}

#[allow(unused)]
impl Screen {
    pub fn screen_to_world(&self, v: Vec2f) -> Vec2f {
        self.pos + (v - self.offset) / self.scale
    }

    pub fn world_to_screen(&self, v: Vec2f) -> Vec2f {
        (v - self.pos) * self.scale + self.offset
    }

    pub fn screen_to_world_tile(&self, v: Vec2f) -> Vec2i {
        self.screen_to_world(v).convert(|v| v.floor() as i32)
    }

    pub fn world_to_screen_tile(&self, v: Vec2i) -> Vec2f {
        self.world_to_screen(v.convert(|v| v as f32))
    }
}

struct SelectionInventoryItem {}
impl InventoryItem for SelectionInventoryItem {
    fn id(&self) -> DynStaticStr {
        "selection".into()
    }

    fn draw(&self, ctx: &PaintContext) {
        let rect = ctx.rect.shrink2(ctx.rect.size() / 5.0);
        ctx.paint
            .rect_filled(rect, Rounding::none(), Selection::fill_color());
        let rect_corners = [
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
        ];

        let mut shapes = vec![];
        Shape::dashed_line_many(
            &rect_corners,
            Stroke::new(1.0, Selection::border_color()),
            3.0,
            2.0,
            &mut shapes,
        );

        shapes.into_iter().for_each(|s| {
            ctx.paint.add(s);
        });
    }
}

struct WireInventoryItem {}
impl InventoryItem for WireInventoryItem {
    fn id(&self) -> DynStaticStr {
        "wire".into()
    }

    fn draw(&self, ctx: &PaintContext) {
        let color = WireState::False.color();

        let rect1 = Rect::from_center_size(
            ctx.rect.lerp_inside([0.2, 0.2].into()),
            ctx.rect.size() * 0.2,
        );
        let rect2 = Rect::from_center_size(
            ctx.rect.lerp_inside([0.8, 0.8].into()),
            ctx.rect.size() * 0.2,
        );

        ctx.paint
            .line_segment([rect1.center(), rect2.center()], Stroke::new(2.5, color));

        ctx.paint.add(Shape::Path(PathShape {
            points: rotated_rect_shape(rect1, PI * 0.25, rect1.center()),
            closed: true,
            fill: color,
            stroke: Stroke::NONE,
        }));

        ctx.paint.add(Shape::Path(PathShape {
            points: rotated_rect_shape(rect2, PI * 0.25, rect2.center()),
            closed: true,
            fill: color,
            stroke: Stroke::NONE,
        }));
    }
}

struct CircuitInventoryItem {
    preview: Arc<CircuitPreview>,
    id: DynStaticStr,
}
impl InventoryItem for CircuitInventoryItem {
    fn id(&self) -> DynStaticStr {
        self.id.clone()
    }

    fn draw(&self, ctx: &PaintContext) {
        let size = self.preview.describe().size.convert(|v| v as f32);
        let scale = Vec2f::from(ctx.rect.size()) / size;
        let scale = scale.x().min(scale.y());
        let size = size * scale;
        let rect = Rect::from_center_size(ctx.rect.center(), size.into());

        let circ_ctx = PaintContext {
            screen: Screen {
                scale,
                ..ctx.screen
            },
            rect,
            ..*ctx
        };
        self.preview.draw(&circ_ctx, false);
    }
}

fn rotated_rect_shape(rect: Rect, angle: f32, origin: Pos2) -> Vec<Pos2> {
    let mut points = vec![
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];

    let cos = angle.cos();
    let sin = angle.sin();

    for p in points.iter_mut() {
        let pl = *p - origin;

        let x = cos * pl.x - sin * pl.y;
        let y = sin * pl.x + cos * pl.y;
        *p = pos2(x, y) + origin.to_vec2();
    }

    points
}

trait Intersect {
    fn intersect(&self, other: &Self) -> Self;
}

impl<T: Ord + Copy> Intersect for Range<T> {
    fn intersect(&self, other: &Self) -> Self {
        Self {
            start: self.start.max(other.start),
            end: self.end.min(other.end),
        }
    }
}

pub trait Integer: Eq + Copy {
    const SIGNED: bool;
    const ZERO: Self;
    const MAX: Self;
    const MIN: Self;
}

macro_rules! impl_integer_trait {
    (signed $($t:ty),+) => {
        $(impl crate::Integer for $t {
            const SIGNED: bool = true;
            const ZERO: $t = 0;
            const MIN: $t = <$t>::MIN;
            const MAX: $t = <$t>::MAX;
        })+
    };
    (unsigned $($t:ty),+) => {
        $(impl crate::Integer for $t {
            const SIGNED: bool = false;
            const ZERO: $t = 0;
            const MIN: $t = <$t>::MIN;
            const MAX: $t = <$t>::MAX;
        })+
    };
}

impl_integer_trait!(signed i8, i16, i32, i64, i128, isize);
impl_integer_trait!(unsigned u8, u16, u32, u64, u128, usize);

macro_rules! impl_optional_int {
    ($name:ident, $none:expr) => {
        #[derive(Clone, Copy, Debug)]
        pub struct $name<T: Integer>(T);

        impl<T: Integer> Default for $name<T> {
            fn default() -> Self {
                Self(Self::NONE_VALUE)
            }
        }

        #[allow(unused)]
        impl<T: Integer> $name<T> {
            const NONE_VALUE: T = $none;

            pub fn new(value: T) -> Self {
                Self(value)
            }

            pub fn none() -> Self {
                Self(Self::NONE_VALUE)
            }

            pub fn is_none(&self) -> bool {
                self.0 == Self::NONE_VALUE
            }

            pub fn is_some(&self) -> bool {
                self.0 != Self::NONE_VALUE
            }

            pub fn is_some_and(&self, f: impl FnOnce(T) -> bool) -> bool {
                self.0 != Self::NONE_VALUE && f(self.0)
            }

            pub fn is_none_or(&self, f: impl FnOnce(T) -> bool) -> bool {
                self.0 == Self::NONE_VALUE || f(self.0)
            }

            pub fn get(&self) -> Option<T> {
                if self.0 == Self::NONE_VALUE {
                    None
                } else {
                    Some(self.0)
                }
            }

            pub fn set(&mut self, value: Option<T>) {
                self.0 = match value {
                    None => Self::NONE_VALUE,
                    Some(v) => v,
                }
            }
        }
    };
}

impl_optional_int!(OptionalInt, (if T::SIGNED { T::MIN } else { T::MAX }));
impl_optional_int!(OptionalNonzeroInt, (T::ZERO));

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Direction4 {
    Up,
    Left,
    Down,
    Right,
}

impl<'de> Deserialize<'de> for Direction4 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Direction4::from_char(char::deserialize(deserializer)?).unwrap_or(Direction4::Up))
    }
}

impl Serialize for Direction4 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.into_char().serialize(serializer)
    }
}

#[allow(unused)]
impl Direction4 {
    pub fn iter_all() -> impl Iterator<Item = Self> {
        [Self::Left, Self::Up, Self::Right, Self::Down].into_iter()
    }

    pub fn unit_vector(self) -> Vec2i {
        match self {
            Self::Up => [0, -1],
            Self::Left => [-1, 0],
            Self::Down => [0, 1],
            Self::Right => [1, 0],
        }
        .into()
    }

    pub fn move_vector(self, vec: Vec2i, distance: i32) -> Vec2i {
        vec + self.unit_vector() * distance
    }

    pub const fn is_vertical(self) -> bool {
        match self {
            Self::Left | Self::Right => false,
            Self::Up | Self::Down => true,
        }
    }

    pub const fn is_horizontal(self) -> bool {
        match self {
            Self::Left | Self::Right => true,
            Self::Up | Self::Down => false,
        }
    }

    pub const fn is_left_up(self) -> bool {
        match self {
            Self::Left | Self::Up => true,
            Self::Right | Self::Down => false,
        }
    }

    pub const fn is_right_bottom(self) -> bool {
        match self {
            Self::Right | Self::Down => true,
            Self::Left | Self::Up => false,
        }
    }

    pub const fn inverted(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Left => Self::Right,
            Self::Down => Self::Up,
            Self::Right => Self::Left,
        }
    }

    pub const fn inverted_ud(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Left => Self::Left,
            Self::Down => Self::Up,
            Self::Right => Self::Right,
        }
    }

    pub const fn inverted_lr(self) -> Self {
        match self {
            Self::Up => Self::Up,
            Self::Left => Self::Right,
            Self::Down => Self::Down,
            Self::Right => Self::Left,
        }
    }

    /// Returns: (direction, forward)
    pub const fn into_dir2(self) -> (Direction2, bool) {
        match self {
            Self::Up => (Direction2::Up, true),
            Self::Left => (Direction2::Left, true),
            Self::Down => (Direction2::Up, false),
            Self::Right => (Direction2::Left, false),
        }
    }

    /// if include_start { returns dist values } else { returns start pos + dist values }
    pub fn iter_pos_along(
        self,
        pos: Vec2i,
        dist: i32,
        include_start: bool,
    ) -> DirectionPosItreator {
        let dir = self.unit_vector() * if dist >= 0 { 1 } else { -1 };
        let dist = dist.unsigned_abs();

        let (pos, dist) = if include_start {
            (pos, dist + 1)
        } else {
            (pos + dir, dist)
        };
        DirectionPosItreator {
            pos,
            remaining: dist,
            dir,
        }
    }

    pub const fn rotate_clockwise(self) -> Self {
        match self {
            Self::Up => Self::Right,
            Self::Left => Self::Up,
            Self::Down => Self::Left,
            Self::Right => Self::Down,
        }
    }

    pub const fn rotate_counterclockwise(self) -> Self {
        match self {
            Self::Up => Self::Left,
            Self::Left => Self::Down,
            Self::Down => Self::Right,
            Self::Right => Self::Up,
        }
    }

    pub const fn into_char(self) -> char {
        match self {
            Direction4::Up => 'u',
            Direction4::Left => 'l',
            Direction4::Down => 'd',
            Direction4::Right => 'r',
        }
    }

    pub const fn from_char(char: char) -> Option<Self> {
        match char {
            'u' => Some(Direction4::Up),
            'l' => Some(Direction4::Left),
            'd' => Some(Direction4::Down),
            'r' => Some(Direction4::Right),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Direction4::Up => "Up",
            Direction4::Left => "Left",
            Direction4::Down => "Down",
            Direction4::Right => "Right",
        }
    }

    pub fn angle_to_right(self) -> f32 {
        match self {
            Direction4::Right => TAU * 0.0,
            Direction4::Up => TAU * 0.25,
            Direction4::Left => TAU * 0.5,
            Direction4::Down => TAU * 0.75,
        }
    }

    pub fn angle_to_left(self) -> f32 {
        match self {
            Direction4::Left => TAU * 0.0,
            Direction4::Down => TAU * 0.25,
            Direction4::Right => TAU * 0.5,
            Direction4::Up => TAU * 0.75,
        }
    }

    // Up - no rotation, Right - one, Down - two, etc
    pub const fn rotate_clockwise_by(self, other: Direction4) -> Self {
        match other {
            Direction4::Up => self,
            Direction4::Right => self.rotate_clockwise(),
            Direction4::Down => self.inverted(),
            Direction4::Left => self.rotate_counterclockwise(),
        }
    }

    // Up - no rotation, Left - one, Down - two, etc
    pub const fn rotate_counterclockwise_by(self, other: Direction4) -> Self {
        match other {
            Direction4::Up => self,
            Direction4::Left => self.rotate_clockwise(),
            Direction4::Down => self.inverted(),
            Direction4::Right => self.rotate_counterclockwise(),
        }
    }
}

impl From<Direction2> for Direction4 {
    fn from(value: Direction2) -> Self {
        match value {
            Direction2::Up => Direction4::Up,
            Direction2::Left => Direction4::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction2 {
    Up,
    Left,
}

impl Direction2 {
    pub fn iter_all() -> impl Iterator<Item = Direction2> {
        [Direction2::Left, Direction2::Up].into_iter()
    }

    pub fn unit_vector(self, forward: bool) -> Vec2i {
        match (self, forward) {
            (Direction2::Up, true) => [0, -1],
            (Direction2::Left, true) => [-1, 0],
            (Direction2::Up, false) => [0, 1],
            (Direction2::Left, false) => [1, 0],
        }
        .into()
    }

    pub fn move_vector(self, vec: Vec2i, distance: i32, forward: bool) -> Vec2i {
        vec + self.unit_vector(forward) * distance
    }

    pub fn choose_axis_component<T>(self, x: T, y: T) -> T {
        match self {
            Direction2::Up => y,
            Direction2::Left => x,
        }
    }

    /// if include_start { returns dist values } else { returns start pos + dist values }
    pub fn iter_pos_along(
        self,
        pos: Vec2i,
        dist: i32,
        include_start: bool,
    ) -> DirectionPosItreator {
        let dir = self.unit_vector(dist >= 0);
        let dist = dist.unsigned_abs();

        let (pos, dist) = if include_start {
            (pos, dist + 1)
        } else {
            (pos + dir, dist)
        };
        DirectionPosItreator {
            pos,
            remaining: dist,
            dir,
        }
    }
}

pub struct DirectionPosItreator {
    pos: Vec2i,
    remaining: u32,
    dir: Vec2i,
}

impl Iterator for DirectionPosItreator {
    type Item = Vec2i;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let p = self.pos;
        self.pos += self.dir;
        self.remaining -= 1;
        Some(p)
    }
}

#[derive(Clone)]
pub enum DynStaticStr {
    Static(&'static str),
    Dynamic(Arc<str>),
}

impl std::fmt::Debug for DynStaticStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(s) => f.write_fmt(format_args!("static \"{s}\"")),
            Self::Dynamic(s) => f.write_fmt(format_args!("dynamic \"{}\"", s.deref())),
        }
    }
}

impl Serialize for DynStaticStr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.deref())
    }
}

impl<'de> Deserialize<'de> for DynStaticStr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let str = <_>::deserialize(deserializer)?;
        Ok(Self::Dynamic(GLOBAL_STR_CACHE.cache(str)))
    }
}

impl Hash for DynStaticStr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.deref().hash(state)
    }
}

impl Eq for DynStaticStr {}

impl PartialEq<str> for DynStaticStr {
    fn eq(&self, other: &str) -> bool {
        self.deref() == other
    }
}

impl<T: PartialEq<str>> PartialEq<T> for DynStaticStr {
    fn eq(&self, other: &T) -> bool {
        other.eq(self.deref())
    }
}

impl Deref for DynStaticStr {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            DynStaticStr::Static(str) => str,
            DynStaticStr::Dynamic(arc) => arc.deref(),
        }
    }
}

impl Borrow<str> for DynStaticStr {
    fn borrow(&self) -> &str {
        self.deref()
    }
}

impl From<&'static str> for DynStaticStr {
    fn from(value: &'static str) -> Self {
        Self::Static(value)
    }
}

impl From<Arc<str>> for DynStaticStr {
    fn from(value: Arc<str>) -> Self {
        Self::Dynamic(value)
    }
}

pub struct PastePreview {
    wires: Vec<crate::io::WirePartCopyData>,
    circuits: Vec<(crate::io::CircuitCopyData, CircuitPreview)>,
    size: Vec2u,
}

impl PastePreview {
    pub fn new(data: crate::io::CopyPasteData, ctx: &impl io::LoadingContext) -> Self {
        let wires = data.wires;
        let circuits: Vec<_> = data
            .circuits
            .into_iter()
            .filter_map(|d| {
                ctx.get_circuit_preview(&d.ty)
                    .and_then(|p| p.load_new(&d.imp, &d.props).map(|b| (d, b)))
            })
            .collect();

        let size = {
            let mut size = Vec2u::default();
            for wire in wires.iter() {
                size = [
                    size.x().max(wire.pos.x() + 1),
                    size.y().max(wire.pos.y() + 1),
                ]
                .into()
            }
            for (circuit, preview) in circuits.iter() {
                let br = circuit.pos + preview.describe().size;
                size = [size.x().max(br.x()), size.y().max(br.y())].into()
            }
            size
        };

        Self {
            wires,
            circuits,
            size,
        }
    }

    pub fn draw(&self, board: &ActiveCircuitBoard, pos: Vec2i, ctx: &PaintContext) {
        let rect = Rect::from_min_size(
            ctx.screen.world_to_screen_tile(pos).into(),
            (self.size.convert(|v| v as f32) * ctx.screen.scale).into(),
        );
        ctx.paint.rect_filled(
            rect,
            Rounding::none(),
            Color32::from_rgba_unmultiplied(0, 120, 120, 120),
        );

        for wire in self.wires.iter() {
            if let Some(length) = NonZeroU32::new(wire.length) {
                let part = WirePart {
                    pos: pos + wire.pos.convert(|v| v as i32),
                    length,
                    dir: wire.dir,
                };
                board.draw_wire_part(ctx, &part, Color32::from_gray(128))
            }
        }

        for (circuit, preview) in self.circuits.iter() {
            let size = preview.describe().size;
            if size.x() == 0 || size.y() == 0 {
                return;
            }
            let pos = pos + circuit.pos.convert(|v| v as i32);
            let rect = Rect::from_min_size(
                ctx.screen.world_to_screen_tile(pos).into(),
                (size.convert(|v| v as f32) * ctx.screen.scale).into(),
            );
            preview.draw(&ctx.with_rect(rect), true);
        }
    }

    fn place(&self, board: &mut ActiveCircuitBoard, pos: Vec2i) {
        if self.circuits.iter().any(|(c, p)| {
            !board.can_place_circuit_at(p.describe().size, pos + c.pos.convert(|v| v as i32), None)
        }) {
            return;
        }

        let sim_lock = { board.board.read().sim_lock.clone() };
        let sim_lock = sim_lock.write();

        let mut wire_ids: HashSet<usize> = HashSet::new();
        for wire in self.wires.iter() {
            if let Some(length) = NonZeroU32::new(wire.length) {
                let part = WirePart {
                    pos: pos + wire.pos.convert(|v| v as i32),
                    length,
                    dir: wire.dir,
                };
                if let Some(id) = board.place_wire_part(part, false) {
                    wire_ids.insert(id);
                }
            }
        }
        for (circuit_data, preview) in self.circuits.iter() {
            let id = board.place_circuit(
                pos + circuit_data.pos.convert(|v| v as i32),
                false,
                preview,
                None,
                &|board, id| {
                    let board = board.board.read();
                    if let Some(circuit) = board.circuits.get(id) {
                        if !matches!(
                            circuit_data.internal,
                            serde_intermediate::Intermediate::Unit
                        ) {
                            for state in board.states.states().read().iter() {
                                let state = state.get_circuit(id);
                                let mut state = state.write();

                                state.internal =
                                    circuit.imp.write().load_internal(&circuit_data.internal);
                            }
                        }

                        if !matches!(circuit_data.imp, serde_intermediate::Intermediate::Unit) {
                            circuit.imp.write().load(&circuit_data.imp)
                        }
                    }
                },
            );
            if let (Some(id), Some(dur)) = (id, circuit_data.update) {
                let board = board.board.read();
                for state in board.states.states().read().iter() {
                    state.set_circuit_update_interval(id, Some(dur))
                }
            }
        }

        let states = board.board.read().states.clone();
        for wire in wire_ids {
            states.update_wire(wire, true);
        }
        drop(sim_lock)
    }
}

#[derive(Default)]
pub struct ArcString {
    string: Option<String>,
    arc: RwLock<Option<Arc<str>>>,
    check_str: AtomicBool,
}

impl Clone for ArcString {
    fn clone(&self) -> Self {

        if self.string.is_none() && self.arc.read().is_none() {
            return Default::default();
        }


        Self {
            string: None,
            arc: RwLock::new(Some(self.get_arc())),
            check_str: AtomicBool::new(self.check_str.load(Ordering::Relaxed)),
        }
    }
}

impl ArcString {
    fn check_string(&self, s: &str) -> bool {
        let check_str = self.check_str.load(Ordering::Relaxed);

        if !check_str {
            return true;
        }
        let str = match &self.string {
            Some(s) => s.as_str(),
            None => "",
        };
        if str == s {
            self.check_str.store(false, Ordering::Relaxed);
            return true;
        }
        false
    }

    pub fn get_arc(&self) -> Arc<str> {
        let arc = self.arc.read().clone();
        if let Some(arc) = arc {
            if self.check_string(&arc) {
                return arc;
            }
        }

        let mut arc = self.arc.write();
        if let Some(arc) = arc.clone() {
            if self.check_string(&arc) {
                return arc;
            }
        }

        self.check_str.store(false, Ordering::Relaxed);
        let str = match &self.string {
            Some(s) => s,
            None => "",
        };
        let new_arc = Arc::<str>::from(str);

        *arc = Some(new_arc.clone());
        new_arc
    }

    pub fn get_mut(&mut self) -> &mut String {
        self.check_str.store(true, Ordering::Relaxed);
        self.string.get_or_insert_with(|| self.arc.read().as_ref().map(|a| a.deref().into()).unwrap_or_default())
    }

    pub fn get_str(&self) -> ArcBorrowStr<'_> {
        if let Some(string) = &self.string {
            ArcBorrowStr::Borrow(string)
        }
        else if let Some(arc) = self.arc.read().as_ref() {
            ArcBorrowStr::Arc(arc.clone())
        }
        else {
            ArcBorrowStr::Borrow("")
        }
    }
}

impl From<&str> for ArcString {
    fn from(value: &str) -> Self {
        Self {
            string: Some(value.into()),
            arc: RwLock::new(None),
            check_str: AtomicBool::new(false),
        }
    }
}

pub enum ArcBorrowStr<'a> {
    Arc(Arc<str>),
    Borrow(&'a str)
}

impl<'a> Deref for ArcBorrowStr<'a> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            ArcBorrowStr::Arc(a) => a.deref(),
            ArcBorrowStr::Borrow(b) => b,
        }
    }
}