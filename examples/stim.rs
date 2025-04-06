use std::time::Duration;

use dungeonctl::{
    Coyote3, Stereo,
    coyote3::{DeviceSettings, IntensityChange, Pulse, Pulses},
};
use futures_signals::signal::SignalExt;
use tracing::info;

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
            limit: Stereo { a: 50, b: 0 },
            ..Default::default()
        })
        .await?;

    async fn stim(coyote: &Coyote3) -> eyre::Result<()> {
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

        for i in 0u64.. {
            coyote
                .send_pulses(Pulses {
                    intensity: Stereo {
                        a: IntensityChange::DoNotChange,
                        b: IntensityChange::DoNotChange,
                    },
                    pulses: [Stereo {
                        a: Pulse {
                            frequency: 200,
                            intensity: 50
                                + (50.0
                                    * ((std::f32::consts::TAU * (i as f32) / 20.0).sin() / 2.0
                                        + 0.5)) as u8,
                        },
                        b: Pulse {
                            frequency: 0,
                            intensity: 0,
                        },
                    }; 4],
                })
                .await?;

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(())
    }

    tokio::select!(
        _ = coyote.state().for_each(async |state| info!(?state)) => Ok(()),
        res = async {
            stim(&coyote).await?;

            coyote.disconnect().await?;

            Ok(())
        } => res,
        res = async {
            tokio::signal::ctrl_c()
                .await?;

            coyote.disconnect().await?;

            Ok(())
        } => res,
    )
}
