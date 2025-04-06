//! Implemention of the Bluetooth LE protocols to control the DG-LAB Coyote 3.

use std::ops::Deref;

use arrayvec::ArrayVec;
use binrw::BinRead;
use btleplug::{
    api::{Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, WriteType},
    platform::{Adapter, Manager, Peripheral},
};
use futures::{FutureExt, StreamExt, future::BoxFuture};
use smart_default::SmartDefault;
use tracing::{debug, error};
use uuid::{Uuid, uuid};

use crate::{
    Error, Result,
    core::{DeviceState, PeripheralExt, StateSignal, Stereo},
};

const DEVICE_NAME: &str = "47L121000";
// const BATTERY_SERVICE_UUID: Uuid = uuid!("0000180A-0000-1000-8000-00805f9b34fb");
// const MAIN_SERVICE_UUID: Uuid = uuid!("0000180C-0000-1000-8000-00805f9b34fb");
const WRITE_CHARACTERISTIC_UUID: Uuid = uuid!("0000150A-0000-1000-8000-00805f9b34fb");
const NOTIFY_CHARACTERISTIC_UUID: Uuid = uuid!("0000150B-0000-1000-8000-00805f9b34fb");
const BATTERY_CHARACTERISTIC_UUID: Uuid = uuid!("00001500-0000-1000-8000-00805f9b34fb");

/// Implements the Bluetooth LE protocols to control the DG-LAB Coyote 3.
///
/// Based on <https://github.com/DG-LAB-OPENSOURCE/DG-LAB-OPENSOURCE/blob/main/coyote/v3/README_V3.md> (Chinese).
#[derive(Debug)]
pub struct Coyote3 {
    peripheral: Peripheral,
    write: Characteristic,
    state: DeviceState<State>,
}
impl Coyote3 {
    /// Connect to a Coyote 3.
    ///
    /// # Examples
    ///
    /// Connect to the first Coyote 3 that could be found using the first BLE adapter that could be found.
    ///
    /// ```no_run
    /// # use dungeonctl::Coyote3;
    /// # #[tokio::main]
    /// # async fn main() -> eyre::Result<()> {
    /// Coyote3::connect().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Connect to a specific Coyote 3 device using a specific BLE adapter and specific settings.
    ///
    /// ```ignore
    /// Coyote3::connect()
    ///     // `adapter` must be a `btleplug::platform::Adapter`
    ///     .with(adapter)
    ///     // `peripheral` must be a `btleplug::platform::Peripheral`
    ///     .to(peripheral)
    ///     .settings(DeviceSettings {
    ///         limit: Stereo { a: 50, b: 0 },
    ///         ..Default::default()
    ///     })
    ///     .await?;
    /// ```
    pub fn connect() -> Coyote3Builder {
        Coyote3Builder::default()
    }
    /// Disconnect from the Coyote3.
    pub async fn disconnect(&self) -> Result<()> {
        self.peripheral.disconnect().await?;

        Ok(())
    }
}

/// Builder type to connect to a Coyote 3.
///
/// This type implements [`IntoFuture`], so you just need to `.await` it to start the connection.
#[derive(Debug, Default)]
pub struct Coyote3Builder {
    adapter: Option<Adapter>,
    peripheral: Option<Peripheral>,
    settings: DeviceSettings,
}

impl Coyote3Builder {
    /// Connect using a specific [`btleplug::platform::Adapter`].
    pub fn with(mut self, adapter: impl Into<Adapter>) -> Self {
        self.adapter = Some(adapter.into());
        self
    }
    /// Connect to a specific [`btleplug::platform::Peripheral`].
    pub fn to(mut self, peripheral: impl Into<Peripheral>) -> Self {
        self.peripheral = Some(peripheral.into());
        self
    }
    /// Set the device settings.
    pub fn settings(mut self, settings: DeviceSettings) -> Self {
        self.settings = settings;
        self
    }
    async fn connect(self) -> Result<Coyote3> {
        let adapter = match self.adapter {
            Some(adapter) => adapter,
            None => {
                let manager = Manager::new().await.unwrap();
                manager.adapters().await?.swap_remove(0)
            }
        };
        let peripheral = match self.peripheral {
            Some(peripheral) => peripheral,
            None => {
                adapter.start_scan(Default::default()).await?;

                let peripheral = 'peripheral: {
                    let mut events = adapter.events().await?;

                    while let Some(event) = events.next().await {
                        if let CentralEvent::DeviceDiscovered(id) = event {
                            let peripheral = adapter.peripheral(&id).await?;
                            if peripheral.local_name_matches(DEVICE_NAME).await? {
                                break 'peripheral peripheral;
                            }
                        }
                    }

                    unreachable!()
                };

                adapter.stop_scan().await?;

                peripheral
            }
        };

        let settings = self.settings;

        debug!("connecting to {}", peripheral.address());
        peripheral.connect().await?;
        debug!("discovering services");
        peripheral.discover_services().await?;

        let mut battery = None;
        let mut write = None;

        for characteristic in peripheral.characteristics() {
            match characteristic.uuid {
                BATTERY_CHARACTERISTIC_UUID => {
                    peripheral.subscribe(&characteristic).await?;
                    battery = Some(characteristic);
                }
                NOTIFY_CHARACTERISTIC_UUID => {
                    peripheral.subscribe(&characteristic).await?;
                }
                WRITE_CHARACTERISTIC_UUID => {
                    write = Some(characteristic);
                }
                _ => {}
            }
        }

        let battery = battery.ok_or(Error::MissingCharacteristic(WRITE_CHARACTERISTIC_UUID))?;
        let write = write.ok_or(Error::MissingCharacteristic(WRITE_CHARACTERISTIC_UUID))?;

        let state = State {
            battery: {
                let value = peripheral.read(&battery).await?;
                debug_assert_eq!(value.len(), 1);
                value[0]
            },
            settings,
            intensity: Stereo { a: 0, b: 0 },
        };

        let state = DeviceState::new(
            peripheral.notifications().await?.filter_map({
                use std::future::ready;

                let mut state = state;

                move |notification| {
                    debug!(?notification);
                    match notification.uuid {
                        NOTIFY_CHARACTERISTIC_UUID => {
                            match Notification::read_be(&mut binrw::io::NoSeek::new(
                                &*notification.value,
                            )) {
                                Ok(Notification::IntensityChange {
                                    serial: _,
                                    intensity,
                                }) => {
                                    state.intensity = intensity;
                                    ready(Some(state))
                                }
                                Ok(Notification::DeviceSettingsChange(parameters)) => {
                                    state.settings = parameters;
                                    ready(Some(state))
                                }
                                Err(e) => {
                                    error!(?e);
                                    ready(None)
                                }
                            }
                        }
                        BATTERY_CHARACTERISTIC_UUID => {
                            debug_assert_eq!(notification.value.len(), 1);
                            state.battery = notification.value[0];
                            ready(Some(state))
                        }
                        uuid => {
                            debug!("received notification for unknown characteristic {uuid}");
                            ready(None)
                        }
                    }
                }
            }),
            state,
        );

        let coyote = Coyote3 {
            peripheral: peripheral.clone(),
            write,
            state,
        };

        coyote.update_settings(settings).await?;

        Ok(coyote)
    }
}

impl IntoFuture for Coyote3Builder {
    type IntoFuture = BoxFuture<'static, Self::Output>;
    type Output = Result<Coyote3>;

    fn into_future(self) -> Self::IntoFuture {
        self.connect().boxed()
    }
}

impl Coyote3 {
    /// Get the state of the connected Coyote3.
    ///
    /// This returns a reactive signal that can either be
    /// used via the [`SignalExt`](futures_signals::signal::SignalExt) trait or the current value
    /// can be obtained using its [`get()`](crate::StateSignal::get) method.
    pub fn state(&self) -> impl StateSignal<State> {
        self.state.clone()
    }
    /// Send the next pulses to the Coyote 3.
    ///
    /// This is expected to be called every 100 ms and
    /// provides the signal data for the next four 25 ms pulses.
    pub async fn send_pulses(&self, pulses: Pulses) -> Result<()> {
        self.send_command(Command::SendPulses(pulses)).await
    }
    /// Update the device settings.
    pub async fn update_settings(&self, settings: DeviceSettings) -> Result<()> {
        self.send_command(Command::UpdateSettings(settings)).await
    }
    async fn send_command(&self, command: Command) -> Result<()> {
        debug!(?command);
        self.peripheral
            .write(&self.write, &command.to_bytes(), WriteType::WithoutResponse)
            .await?;

        Ok(())
    }
}

/// The current state of the Coyote 3. This can be obtained by calling [`Coyote3::state()`].
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct State {
    /// The current battery charge in percent.
    pub battery: u8,
    /// The current stimulation intensity.
    pub intensity: Stereo<u8>,
    /// The current device settings.
    pub settings: DeviceSettings,
}

/// The device settings of the Coyote 3.
#[derive(Clone, Copy, Debug, PartialEq, SmartDefault, binrw::BinRead, binrw::BinWrite)]
#[brw(big)]
pub struct DeviceSettings {
    /// The maximum intensity limit.
    ///
    /// <div class="warning">It is very important that a user can set this to appropriate levels.</div>
    #[default((70, 70).into())]
    pub limit: Stereo<u8>,

    /// The “frequency balance” parameter affects the perceived intensity at different frequencies.
    ///
    /// The official app explains it as following:
    ///
    /// > This parameter controls the relative intensity of waveforms at different frequencies,
    /// > under a fixed channel intensity. Higher values increase the throbbing sensation of
    /// > low-frequency waveforms.
    #[default((160, 160).into())]
    pub frequency_balance: Stereo<u8>,

    /// The “intensity balance” parameter affects the pulse width of the waveform.
    /// Whether this parameter actually influences the waveform is currently questionable.
    ///
    /// The official app explains it as following:
    ///
    /// > This parameter controls the relative intensity of waveforms at different frequencies,
    /// > under a fixed channel intensity. Higher values increase the perceived stimulation of
    /// > low-frequency waveforms.
    #[default((0, 0).into())]
    pub intensity_balance: Stereo<u8>,
}

/// The pulse data that is expected to be sent every 100 ms to the coyote.
#[derive(Clone, Copy, Debug, binrw::BinWrite)]
#[bw(big)]
pub struct Pulses {
    /// This field is used to change the stimulation intensity per channel.
    ///
    /// Note that relative changes should be preferred in many cases over absolute changes since
    /// absolute changes will overwrite any intensity changes that were made using the hardware
    /// “shoulder” switches of the coyote, basically rendering them useless.
    #[bw(map = |intensity| (
        (intensity.a.mode() << 2) | intensity.b.mode(),
        intensity.a.value(),
        intensity.b.value(),
    ))]
    pub intensity: Stereo<IntensityChange>,

    /// The actual waveform data.
    ///
    /// This is an array of 4 pulses of 25 ms length each, where each pulse contains the frequency
    /// and relative amplitude for each channel.
    #[bw(map = Self::convert_pulses)]
    pub pulses: [Stereo<Pulse>; 4],
}

impl Pulses {
    fn convert_pulses(pulses: &[Stereo<Pulse>; 4]) -> [[u8; 4]; 4] {
        [
            pulses.map(|p| p.a.compressed_frequency_value()),
            pulses.map(|p| p.a.clamped_intensity()),
            pulses.map(|p| p.b.compressed_frequency_value()),
            pulses.map(|p| p.b.clamped_intensity()),
        ]
    }
}

/// A single frequency-intensity set representing 25 ms of a waveform for a single channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pulse {
    /// The frequency in Hz in the range of 1 Hz to 100 Hz (official maximum) / 200 Hz (actual maximum)
    pub frequency: u8,
    /// The pulse amplitude as an abstract value in the range of 0 to 100.
    pub intensity: u8,
}

impl Pulse {
    fn compressed_frequency_value(&self) -> u8 {
        if self.frequency == 0 {
            return 0;
        }

        let t = 1000.0 / (self.frequency as f32);

        #[allow(clippy::match_overlapping_arm)]
        let compressed_t = match t {
            ..5.0 => 5.0,
            ..100.0 => t,
            ..600.0 => (t - 100.0) / 5.0 + 100.0,
            ..1000.0 => (t - 600.0) / 10.0 + 200.0,
            _ => 240.0,
        };

        compressed_t as u8
    }
    fn clamped_intensity(&self) -> u8 {
        self.intensity.clamp(0, 100)
    }
}

/// Used to describe if and how the stimulation intensity should be changed.
///
/// Note that relative changes should be preferred in many cases over absolute changes since
/// absolute changes will overwrite any intensity changes that were made using the hardware
/// “shoulder” switches of the coyote, basically rendering them useless.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IntensityChange {
    /// Do not change the intensity.
    DoNotChange,
    /// Increase the intensity by `x`.
    RelativeIncrease(u8),
    /// Decrease the intensity by `x`.
    RelativeDecrease(u8),
    /// Set the intensity to `x`.
    AbsoluteChange(u8),
}

impl IntensityChange {
    fn mode(&self) -> u8 {
        match self {
            IntensityChange::DoNotChange => 0b00,
            IntensityChange::RelativeIncrease(_) => 0b01,
            IntensityChange::RelativeDecrease(_) => 0b10,
            IntensityChange::AbsoluteChange(_) => 0b11,
        }
    }
    fn value(&self) -> u8 {
        match self {
            IntensityChange::DoNotChange => 0,
            IntensityChange::RelativeIncrease(v)
            | IntensityChange::RelativeDecrease(v)
            | IntensityChange::AbsoluteChange(v) => *v,
        }
    }
}

#[derive(Clone, Copy, Debug, binrw::BinWrite)]
#[bw(big)]
enum Command {
    #[bw(magic = 0xB0u8)]
    SendPulses(Pulses),
    #[bw(magic = 0xBFu8)]
    UpdateSettings(DeviceSettings),
}
impl Command {
    fn to_bytes(self) -> impl Deref<Target = [u8]> {
        use binrw::BinWrite;

        let mut buf = ArrayVec::<u8, 20>::new_const();
        self.write_be(&mut binrw::io::NoSeek::new(&mut buf))
            .expect("writing must not fail");
        buf
    }
}
#[derive(Debug, binrw::BinRead)]
#[br(big)]
enum Notification {
    #[br(magic = 0xB1u8)]
    IntensityChange {
        #[allow(dead_code)]
        serial: u8,
        intensity: Stereo<u8>,
    },
    #[br(magic = 0xBEu8)]
    DeviceSettingsChange(DeviceSettings),
}

#[cfg(test)]
mod tests {
    use super::*;

    use hex_literal::hex;

    #[test]
    fn test_b0_command() {
        assert_eq!(
            &*Command::SendPulses(Pulses {
                intensity: Stereo {
                    a: IntensityChange::AbsoluteChange(10),
                    b: IntensityChange::AbsoluteChange(0)
                },
                pulses: [Stereo {
                    a: Pulse {
                        frequency: 100,
                        intensity: 0
                    },
                    b: Pulse {
                        frequency: 30,
                        intensity: 0
                    }
                }; 4]
            })
            .to_bytes(),
            hex!("b00f0a000a0a0a0a000000002121212100000000")
        );
        assert_eq!(
            &*Command::SendPulses(Pulses {
                intensity: Stereo {
                    a: IntensityChange::AbsoluteChange(10),
                    b: IntensityChange::AbsoluteChange(0)
                },
                pulses: [Stereo {
                    a: Pulse {
                        frequency: 100,
                        intensity: 100
                    },
                    b: Pulse {
                        frequency: 30,
                        intensity: 100
                    }
                }; 4]
            })
            .to_bytes(),
            hex!("b00f0a000a0a0a0a646464642121212164646464")
        );
    }

    #[test]
    fn test_bf_command() {
        assert_eq!(
            &*Command::UpdateSettings(DeviceSettings {
                limit: Stereo { a: 200, b: 200 },
                frequency_balance: Stereo { a: 160, b: 160 },
                intensity_balance: Stereo { a: 0, b: 0 },
            })
            .to_bytes(),
            hex!("bfc8c8a0a00000")
        );
    }
}
