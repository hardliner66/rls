use eframe::epaint::{Color32, PathShape, Stroke};
use emath::{vec2, Pos2, pos2};

use crate::{
    circuits::{
        CircuitImpl, CircuitPinInfo, CircuitPreviewImpl, CircuitPropertyStore, CircuitStateContext,
        InternalPinDirection, props::CircuitProperty, CircuitDescription,
    },
    state::WireState,
    vector::{Vec2u, Vec2f},
    Direction4, DynStaticStr, PaintContext, describe_directional_circuit,
};

struct Circuit {
    dir: Direction4,
    input: CircuitPinInfo,
    output: CircuitPinInfo,
}

impl Circuit {
    fn new() -> Self {
        let description = Self::describe(Direction4::Right);
        Self {
            input: description.pins[0].to_info(),
            output: description.pins[1].to_info(),
            dir: Direction4::Right,
        }
    }

    fn draw(ctx: &PaintContext, angle: f32, semi_transparent: bool) {
        let opacity = if semi_transparent { 0.6 } else { 1.0 };

        let border_color = Color32::BLACK.linear_multiply(opacity);
        let fill_color = Color32::from_gray(200).linear_multiply(opacity);

        let size = vec2(2.0, 1.0);
        let transformer = |p: Pos2| {
            ctx.rect.lerp_inside(
                Vec2f::from(p.to_vec2() / size)
                    .rotated_xy(angle, 0.5)
                    .into(),
            )
        };

        let points = vec![
            transformer(pos2(0.5, 0.1)),
            transformer(pos2(1.32, 0.5)),
            transformer(pos2(0.5, 0.9)),
        ];
        ctx.paint.add(PathShape {
            points,
            closed: true,
            fill: fill_color,
            stroke: Stroke::new(0.15 * ctx.screen.scale, border_color),
        });
        ctx.paint.circle(
            transformer(pos2(1.32, 0.5)),
            0.2 * ctx.screen.scale,
            fill_color,
            Stroke::new(0.15 * ctx.screen.scale, border_color),
        );
    }

    fn describe_props(props: &CircuitPropertyStore) -> CircuitDescription<2> {
        let dir = props.read_clone("dir").unwrap_or(Direction4::Right);
        Self::describe(dir)
    }

    fn describe(dir: Direction4) -> CircuitDescription<2> {
        describe_directional_circuit! {
            default_dir: Right,
            dir: dir,
            size: [2, 1],

            "in": Inside, "In", Left, [0, 0],
            "out": Outside, "Out", Right, [1, 0]
        }
    }
}

impl CircuitImpl for Circuit {
    fn draw(&self, _: &CircuitStateContext, paint_ctx: &PaintContext) {
        let angle = self.dir.inverted_ud().angle_to_right();
        Circuit::draw(paint_ctx, angle, false);
    }

    fn create_pins(&mut self, props: &CircuitPropertyStore) -> Box<[CircuitPinInfo]> {
        let description = Circuit::describe_props(props);
        self.input = description.pins[0].to_info();
        self.output = description.pins[1].to_info();
        vec![self.input.clone(), self.output.clone()].into_boxed_slice()
    }

    fn update_signals(&self, state_ctx: &CircuitStateContext, _: Option<usize>) {
        let state = self.input.get_state(state_ctx);
        let state = match state {
            WireState::None => WireState::None,
            WireState::True => WireState::False,
            WireState::False => WireState::True,
            WireState::Error => WireState::Error,
        };
        self.output.set_state(state_ctx, state);
    }

    fn size(&self, props: &CircuitPropertyStore) -> Vec2u {
        Self::describe_props(props).size
    }

    fn prop_changed(&self, prop_id: &str, resize: &mut bool, recreate_pins: &mut bool) {
        if prop_id == "dir" {
            *resize = true;
            *recreate_pins = true;
        }
    }

    fn apply_props(&mut self, props: &CircuitPropertyStore, _: Option<&str>) {
        self.dir = props.read_clone("dir").unwrap_or(Direction4::Right);
    }
}

pub struct Preview {}

impl CircuitPreviewImpl for Preview {
    fn draw_preview(&self, props: &CircuitPropertyStore, ctx: &PaintContext, in_world: bool) {
        let angle = props
            .read_clone("dir")
            .unwrap_or(Direction4::Right)
            .inverted_ud()
            .angle_to_right();
        Circuit::draw(ctx, angle, in_world);
    }

    fn create_impl(&self) -> Box<dyn CircuitImpl> {
        Box::new(Circuit::new())
    }

    fn type_name(&self) -> DynStaticStr {
        "not".into()
    }

    fn load_impl_data(
        &self,
        _: &serde_intermediate::Intermediate,
    ) -> Option<Box<dyn CircuitPreviewImpl>> {
        Some(Box::new(Preview {}))
    }

    fn default_props(&self) -> CircuitPropertyStore {
        CircuitPropertyStore::new([
            CircuitProperty::new("dir", "Direction", Direction4::Right)
        ])
    }

    fn display_name(&self) -> DynStaticStr {
        "NOT gate".into()
    }

    fn describe(&self, props: &CircuitPropertyStore) -> crate::circuits::DynCircuitDescription {
        Circuit::describe_props(props).to_dyn()
    }
}
