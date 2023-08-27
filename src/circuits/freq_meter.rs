use eframe::epaint::{Color32, FontId, Rounding, Stroke};
use emath::{vec2, Align2, Rect};

use crate::{circuits::*, containers::ConstRingBuffer, Direction4};

use super::props::CircuitProperty;

struct State {
    timings: ConstRingBuffer<64, Instant>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            timings: ConstRingBuffer::new(),
        }
    }
}

impl InternalCircuitState for State {}

struct Circuit {
    input: CircuitPinInfo,
}

impl Circuit {
    fn new() -> Self {
        Self {
            input: CircuitPinInfo::new([0, 1], InternalPinDirection::Inside, "pin"),
        }
    }

    fn draw(ctx: &PaintContext, state: Option<&CircuitStateContext>, semi_transparent: bool) {
        let opacity = if semi_transparent { 0.6 } else { 1.0 };

        let border_color = Color32::BLACK.linear_multiply(opacity);
        let fill_color = Color32::from_gray(200).linear_multiply(opacity);

        let rect = Rect::from_min_size(
            ctx.rect.left_top() + ctx.rect.size() * vec2(0.5 / 5.0, 0.5 / 3.0),
            ctx.rect.size() * vec2(4.0 / 5.0, 2.0 / 3.0),
        );
        ctx.paint.rect(
            rect,
            Rounding::none(),
            fill_color,
            Stroke::new(2.0, border_color),
        );

        fn format_freq(f: f32) -> String {

            if f >= 10_000.0 {
                format!("{} kHz", (f/1000.0) as i32)
            }
            else if f >= 1000.0 {
                format!("{:.01} kHz", f / 1000.0)
            }
            else if f >= 100.0 {
                format!("{} Hz", f as i32)
            } else {
                format!("{f:.01} Hz")
            }
        }

        let text: String = match state {
            Some(s) => format_freq(
                s.read_circuit_internal_state(|s: &State| {
                    if s.timings.len() < 2 {
                        return 0.0;
                    }

                    let count = s.timings.len();
                    let first = s.timings[0];
                    let last = s.timings[s.timings.len() - 1];
                    match last.checked_duration_since(first) {
                        None => 0.0,
                        Some(d) => 1.0 / d.as_secs_f32() * count as f32,
                    }
                })
                .unwrap_or(0.0),
            ),
            None => "0 Hz".into(),
        };
        let font = FontId::monospace(ctx.screen.scale * 0.8);
        ctx.paint.text(
            ctx.rect.center(),
            Align2::CENTER_CENTER,
            text,
            font,
            border_color,
        );
    }

    fn size(_: &CircuitPropertyStore) -> Vec2u {
        [5, 3].into()
    }

    fn pin_positions(props: &CircuitPropertyStore) -> [u32; 2] {
        let dir = props.read_clone("dir").unwrap_or(Direction4::Left);
        match dir {
            Direction4::Up => [2, 0],
            Direction4::Left => [0, 1],
            Direction4::Down => [2, 2],
            Direction4::Right => [4, 1],
        }
    }
}

impl CircuitImpl for Circuit {
    fn draw(&self, state_ctx: &CircuitStateContext, paint_ctx: &PaintContext) {
        Circuit::draw(paint_ctx, Some(state_ctx), false);
    }

    fn create_pins(&mut self, props: &CircuitPropertyStore) -> Box<[CircuitPinInfo]> {
        let pin_position = Circuit::pin_positions(props);
        self.input = CircuitPinInfo::new(pin_position, InternalPinDirection::Inside, "pin");
        vec![self.input.clone()].into_boxed_slice()
    }

    fn update_signals(&self, state_ctx: &CircuitStateContext, changed_pin: Option<usize>) {
        if changed_pin == Some(0) && self.input.get_input(state_ctx) == WireState::True {
            state_ctx
                .write_circuit_internal_state(|s: &mut State| s.timings.push_back(Instant::now()))
        }
    }

    fn size(&self, props: &CircuitPropertyStore) -> Vec2u {
        Circuit::size(props)
    }

    fn prop_changed(&self, prop_id: &str, _: &mut bool, recreate_pins: &mut bool) {
        if prop_id == "dir" {
            *recreate_pins = true;
        }
    }
}

#[derive(Debug)]
pub struct Preview {}

impl CircuitPreviewImpl for Preview {
    fn type_name(&self) -> DynStaticStr {
        "freq_meter".into()
    }

    fn draw_preview(&self, props: &CircuitPropertyStore, ctx: &PaintContext, in_world: bool) {
        Circuit::draw(ctx, None, in_world);
        draw_pins_preview(ctx, Circuit::size(props), [Circuit::pin_positions(props)])
    }

    fn create_impl(&self) -> Box<dyn CircuitImpl> {
        Box::new(Circuit::new())
    }

    fn size(&self, props: &CircuitPropertyStore) -> Vec2u {
        Circuit::size(props)
    }

    fn load_impl_data(
        &self,
        _: &serde_intermediate::Intermediate,
    ) -> Option<Box<dyn CircuitPreviewImpl>> {
        Some(Box::new(Preview {}))
    }

    fn default_props(&self) -> CircuitPropertyStore {
        CircuitPropertyStore::new([CircuitProperty::new("dir", "Direction", Direction4::Left)])
    }
}