use dungeonctl::{
    Coyote3, LedColor, Stereo,
    coyote3::{DeviceSettings, IntensityChange, Pulse, Pulses, Shape, ShapeContext, ZeroShape},
};

struct SineShape {
    frame_index: u64,
}

impl SineShape {
    fn new() -> Self {
        Self { frame_index: 0 }
    }
}

impl Shape for SineShape {
    fn next_pulse(&mut self, context: ShapeContext) -> Pulse {
        if context.chunk_index == 0 {
            self.frame_index += 1;
        }

        let wave = (std::f32::consts::TAU * (self.frame_index as f32) / 20.0).sin();

        Pulse {
            frequency: 200,
            intensity: 50 + (50.0 * (wave / 2.0 + 0.5)) as u8,
        }
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .or_else(|_| "info,dungeonctl=debug".parse())?,
        )
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .init();

    let coyote = Coyote3::connect()
        .settings(DeviceSettings {
            limit: Stereo { a: 50, b: 50 },
            ..Default::default()
        })
        .await?;
    coyote
        .send_pulses(Pulses {
            intensity: Stereo {
                a: IntensityChange::AbsoluteChange(20),
                b: IntensityChange::AbsoluteChange(0),
            },
            pulses: [Stereo {
                a: Pulse {
                    frequency: 0,
                    intensity: 0,
                },
                b: Pulse {
                    frequency: 0,
                    intensity: 0,
                },
            }; 4],
        })
        .await?;
    coyote.set_led_color(LedColor::Yellow).await?;
    coyote.set_color(LedColor::Yellow).await?;

    let runtime = coyote.runtime();

    runtime.set_shape_a(SineShape::new());

    runtime.set_shape_b(ZeroShape);

    runtime.start();
    tokio::signal::ctrl_c().await?;
    runtime.stop().await?;

    Ok(())
}
