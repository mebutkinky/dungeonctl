use dungeonctl::{
    PawPrints,
    pawprints11::{MainColor, PawPrintsEvent, Settings, ShoulderColor, TriggerMode},
};
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
    let paw = PawPrints::connect()
        .settings(Settings {
            main_color: MainColor::Blue,
            trigger: TriggerMode::PhysicalData,
        })
        .await?;
    paw.set_shoulder_color(ShoulderColor::Blue).await?;
    paw.run_events(|event| async move {
        match event {
            PawPrintsEvent::PhysicalData {
                accel,
                pressed,
                acceleration,
                external_voltage,
                ..
            } => {
                println!(
                    "physical: pressed={pressed:?} acceleration={acceleration} x={} y={} z={} voltage={external_voltage}",
                    accel.x,
                    accel.y,
                    accel.z,
                );
            }

            other => {
                println!("event: {:?}", other);
            }
        }

        Ok(())
    })
    .await?;
    tokio::signal::ctrl_c().await?;
    paw.disconnect().await?;
    Ok(())
}
