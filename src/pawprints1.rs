//! Legacy implementation of the Bluetooth LE protocols for DG-LAB PawPrints.

use btleplug::{
    api::{Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, WriteType},
    platform::{Adapter, Manager, Peripheral},
};
use futures::{FutureExt, Stream, StreamExt, future::BoxFuture};
use tracing::debug;
use uuid::{Uuid, uuid};

use crate::{Error, LedColor, Result, core::PeripheralExt};

const DEVICE_NAMES: [&str; 2] = ["47L120100", "47L120300"];
const WRITE_CHARACTERISTIC_UUID: Uuid = uuid!("0000150A-0000-1000-8000-00805f9b34fb");
const NOTIFY_CHARACTERISTIC_UUID: Uuid = uuid!("0000150B-0000-1000-8000-00805f9b34fb");

/// Implements the Bluetooth LE protocols to control the DG-LAB PawPrints.
#[derive(Debug)]
pub struct PawPrints {
    peripheral: Peripheral,
    write: Characteristic,
}

impl PawPrints {
    /// Connect to a PawPrints button.
    pub fn connect() -> PawPrintsBuilder {
        PawPrintsBuilder::default()
    }

    /// Disconnect from the button.
    pub async fn disconnect(&self) -> Result<()> {
        self.peripheral.disconnect().await?;
        Ok(())
    }

    /// Apply a PawPrints settings payload.
    ///
    /// The button’s “init” packet is just the [`SettingMode::None`] settings payload with
    /// `selector = 1`, so this method also covers initialization.
    pub async fn update_settings(&self, settings: Settings) -> Result<()> {
        self.send_raw(settings.to_bytes().as_ref()).await
    }

    /// Set the button LED using the separate short `0x70` command family.
    pub async fn set_led_color(&self, led_color: LedColor) -> Result<()> {
        self.send_raw(&[0x70, led_color.raw()]).await
    }

    /// Create an event listener stream for parsed PawPrints notifications.
    ///
    /// This parses the runtime notification characteristic into higher-level events such as
    /// button edges, state frames, motion frames, and unknown payloads.
    pub async fn event_listener(&self) -> Result<impl Stream<Item = PawPrintsEvent> + Send> {
        let notifications = self.peripheral.notifications().await?;

        Ok(notifications.filter_map(|notification| async move {
            (notification.uuid == NOTIFY_CHARACTERISTIC_UUID)
                .then(|| PawPrintsEvent::from_bytes(&notification.value))
        }))
    }

    /// Run the PawPrints notification loop and call an async callback for each parsed event.
    pub async fn run_events<F, Fut>(&self, mut callback: F) -> Result<()>
    where
        F: FnMut(PawPrintsEvent) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let events = self.event_listener().await?;
        futures::pin_mut!(events);

        while let Some(event) = events.next().await {
            callback(event).await?;
        }

        Ok(())
    }

    async fn send_raw(&self, payload: &[u8]) -> Result<()> {
        debug!(payload = %hex_bytes(payload), "writing pawprints payload");
        self.peripheral
            .write(&self.write, payload, WriteType::WithoutResponse)
            .await?;
        Ok(())
    }
}

/// Builder type to connect to a PawPrints button.
#[derive(Debug, Default)]
pub struct PawPrintsBuilder {
    adapter: Option<Adapter>,
    peripheral: Option<Peripheral>,
    settings: Settings,
}

impl PawPrintsBuilder {
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

    /// Apply the given settings immediately after connecting.
    pub fn settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }

    async fn connect(self) -> Result<PawPrints> {
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
                            if matches_device_name(&peripheral).await? {
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

        debug!("connecting to {}", peripheral.address());
        peripheral.connect().await?;
        debug!("discovering services");
        peripheral.discover_services().await?;

        let mut write = None;
        let mut notify = None;

        for characteristic in peripheral.characteristics() {
            match characteristic.uuid {
                WRITE_CHARACTERISTIC_UUID => write = Some(characteristic),
                NOTIFY_CHARACTERISTIC_UUID => {
                    peripheral.subscribe(&characteristic).await?;
                    notify = Some(characteristic);
                }
                _ => {}
            }
        }

        let _notify = notify.ok_or(Error::MissingCharacteristic(NOTIFY_CHARACTERISTIC_UUID))?;
        let write = write.ok_or(Error::MissingCharacteristic(WRITE_CHARACTERISTIC_UUID))?;

        let pawprints = PawPrints {
            peripheral: peripheral.clone(),
            write,
        };

        pawprints.update_settings(self.settings).await?;

        Ok(pawprints)
    }
}

impl IntoFuture for PawPrintsBuilder {
    type IntoFuture = BoxFuture<'static, Self::Output>;
    type Output = Result<PawPrints>;

    fn into_future(self) -> Self::IntoFuture {
        self.connect().boxed()
    }
}

/// A full PawPrints runtime settings payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Selector byte written as byte 1 of the `0x50` settings payload.
    ///
    /// The original app calls this `colorFrom`, but the exact firmware semantics appear to be
    /// broader than just LED color, so this API keeps the more neutral `selector` naming.
    pub selector: LedColor,
    /// The mode-specific configuration.
    pub mode: SettingMode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            selector: LedColor::Yellow,
            mode: SettingMode::None,
        }
    }
}

impl Settings {
    fn to_bytes(self) -> [u8; 17] {
        let mut payload = [0u8; 17];
        payload[0] = 0x50;
        payload[1] = self.selector.raw();

        match self.mode {
            SettingMode::None => {}
            SettingMode::Button {
                button_id,
                event,
                para_speed,
                para_cancel_down,
                para_down_speed,
                para_increase,
            } => {
                payload[2] = 0x01;
                payload[3] = button_id;
                payload[4] = event.raw();
                payload[5] = para_speed;
                payload[6] = para_cancel_down;
                payload[7] = para_down_speed;
                payload[8] = para_increase;
            }
            SettingMode::Multi {
                short_click_id,
                long_click_id,
                acceleration_id,
                acceleration,
                para_mapping,
                debouncing,
                cancel_debouncing,
            } => {
                payload[2] = 0x02;
                payload[3] = short_click_id;
                payload[5] = long_click_id;
                payload[6] = acceleration_id;

                match acceleration {
                    MultiAcceleration::Overall {
                        threshold,
                        greater_than,
                    } => {
                        payload[7] = 0x00;
                        payload[8] = u8::from(greater_than);
                        payload[9..11].copy_from_slice(&threshold.to_be_bytes());
                    }
                    MultiAcceleration::Rotation {
                        threshold,
                        greater_than,
                    } => {
                        payload[7] = 0x01;
                        payload[8] = u8::from(greater_than);
                        payload[9..11].copy_from_slice(&threshold.to_be_bytes());
                    }
                    MultiAcceleration::Fixed {
                        x_low,
                        x_high,
                        y_low,
                        y_high,
                        z_low,
                        z_high,
                    } => {
                        payload[7] = 0x02;
                        payload[8] = x_low as u8;
                        payload[9] = x_high as u8;
                        payload[10] = y_low as u8;
                        payload[11] = y_high as u8;
                        payload[12] = z_low as u8;
                        payload[13] = z_high as u8;
                    }
                }

                payload[14] = para_mapping;
                payload[15] = debouncing;
                payload[16] = cancel_debouncing;
            }
            SettingMode::ExternalVoltage {
                out_voltage_id,
                pullup,
                lower_threshold,
                upper_threshold,
                para_mapping,
            } => {
                payload[2] = 0x0F;
                payload[3] = out_voltage_id;
                payload[4] = u8::from(pullup);
                payload[5] = lower_threshold;
                payload[6] = upper_threshold;
                payload[7] = para_mapping;
            }
            SettingMode::MotionStream => {
                payload[2] = 0xFF;
            }
        }

        payload
    }
}

/// The supported PawPrints settings modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingMode {
    /// Clear all configured trigger behavior.
    None,
    /// Button press/release trigger mode.
    Button {
        /// The configured button identifier.
        button_id: u8,
        /// Whether the trigger reacts to press or release.
        event: ButtonTriggerEvent,
        /// Parameter increase speed.
        para_speed: u8,
        /// Parameter decrease applied when the trigger is canceled.
        para_cancel_down: u8,
        /// Parameter decay speed.
        para_down_speed: u8,
        /// Parameter increase amount.
        para_increase: u8,
    },
    /// Combined short click, long click, and acceleration trigger mode.
    Multi {
        /// Trigger id used for short clicks.
        short_click_id: u8,
        /// Trigger id used for long clicks.
        long_click_id: u8,
        /// Trigger id used for acceleration.
        acceleration_id: u8,
        /// Acceleration sub-mode configuration.
        acceleration: MultiAcceleration,
        /// Parameter mapping upper bound.
        para_mapping: u8,
        /// Trigger debounce time.
        debouncing: u8,
        /// Debounce time used when canceling the trigger.
        cancel_debouncing: u8,
    },
    /// External voltage input trigger mode.
    ExternalVoltage {
        /// Trigger identifier.
        out_voltage_id: u8,
        /// Whether the internal pullup should be enabled.
        pullup: bool,
        /// Lower threshold value.
        lower_threshold: u8,
        /// Upper threshold value.
        upper_threshold: u8,
        /// Parameter mapping upper bound.
        para_mapping: u8,
    },
    /// Special motion-stream command used by the hidden test UI.
    MotionStream,
}

/// Button-edge selection used by [`SettingMode::Button`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonTriggerEvent {
    /// Trigger on button press.
    Press,
    /// Trigger on button release.
    Release,
}

impl ButtonTriggerEvent {
    fn raw(self) -> u8 {
        match self {
            Self::Press => 0,
            Self::Release => 1,
        }
    }
}

/// Acceleration configuration used by [`SettingMode::Multi`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiAcceleration {
    /// Overall acceleration threshold.
    Overall {
        /// Threshold value in big-endian `u16` form on the wire.
        threshold: u16,
        /// Whether the threshold comparison is “greater than”.
        greater_than: bool,
    },
    /// Rotation/angle threshold.
    Rotation {
        /// Threshold value in big-endian `u16` form on the wire.
        threshold: u16,
        /// Whether the threshold comparison is “greater than”.
        greater_than: bool,
    },
    /// Fixed XYZ window thresholds.
    Fixed {
        /// Minimum X threshold.
        x_low: i8,
        /// Maximum X threshold.
        x_high: i8,
        /// Minimum Y threshold.
        y_low: i8,
        /// Maximum Y threshold.
        y_high: i8,
        /// Minimum Z threshold.
        z_low: i8,
        /// Maximum Z threshold.
        z_high: i8,
    },
}

/// A parsed PawPrints notification event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PawPrintsEvent {
    /// A short button-press edge packet.
    ButtonPressed {
        /// Selector/context byte returned by the firmware.
        selector: LedColor,
    },
    /// A short button-release edge packet.
    ButtonReleased {
        /// Selector/context byte returned by the firmware.
        selector: LedColor,
    },
    /// A 13-byte state packet with digital state and motion values.
    State {
        /// Bottom first button state. `true` means the raw value was not `1`.
        bottom_first: bool,
        /// Bottom second button state. `true` means the raw value was not `1`.
        bottom_second: bool,
        /// Top button state. `true` means the raw value was not `1`.
        top: bool,
        /// Overall acceleration value.
        overall: i16,
        /// Rotation/angle value.
        rotation: i16,
        /// X-axis value.
        x: i16,
        /// Y-axis value.
        y: i16,
        /// Z-axis value.
        z: i16,
    },
    /// A `0x48` motion packet.
    Motion {
        /// Sound value.
        sound: i16,
        /// X-axis value.
        x: i16,
        /// Y-axis value.
        y: i16,
        /// Z-axis value.
        z: i16,
    },
    /// A `0xF1` debug packet.
    DebugF1 {
        /// X-down threshold value.
        xd: i16,
        /// X-up threshold value.
        xu: i16,
        /// Y-down threshold value.
        yd: i16,
        /// Y-up threshold value.
        yu: i16,
        /// Z-down threshold value.
        zd: i16,
        /// Z-up threshold value.
        zu: i16,
    },
    /// A packet that is not recognized yet.
    Unknown(Vec<u8>),
}

impl PawPrintsEvent {
    fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() == 4 && bytes[0] == 0x5A && bytes[2] == 0x01 && bytes[3] == 0x01 {
            return Self::ButtonPressed {
                selector: bytes[1].into(),
            };
        }
        if bytes.len() == 3 && bytes[0] == 0x5B && bytes[2] == 0x01 {
            return Self::ButtonReleased {
                selector: bytes[1].into(),
            };
        }
        if bytes.len() == 13 && matches!(bytes[0], 0x00 | 0x01) {
            return Self::State {
                bottom_first: bytes[0] != 1,
                bottom_second: bytes[1] != 1,
                top: bytes[2] != 1,
                overall: i16::from_be_bytes([bytes[3], bytes[4]]),
                rotation: i16::from_be_bytes([bytes[5], bytes[6]]),
                x: i16::from_be_bytes([bytes[7], bytes[8]]),
                y: i16::from_be_bytes([bytes[9], bytes[10]]),
                z: i16::from_be_bytes([bytes[11], bytes[12]]),
            };
        }
        if bytes.len() == 9 && bytes[0] == 0x48 {
            return Self::Motion {
                sound: i16::from_be_bytes([bytes[1], bytes[2]]),
                x: i16::from_be_bytes([bytes[3], bytes[4]]),
                y: i16::from_be_bytes([bytes[5], bytes[6]]),
                z: i16::from_be_bytes([bytes[7], bytes[8]]),
            };
        }
        if bytes.len() == 14 && bytes[0] == 0xF1 {
            return Self::DebugF1 {
                xd: i16::from_be_bytes([bytes[2], bytes[3]]),
                xu: i16::from_be_bytes([bytes[4], bytes[5]]),
                yd: i16::from_be_bytes([bytes[6], bytes[7]]),
                yu: i16::from_be_bytes([bytes[8], bytes[9]]),
                zd: i16::from_be_bytes([bytes[10], bytes[11]]),
                zu: i16::from_be_bytes([bytes[12], bytes[13]]),
            };
        }

        Self::Unknown(bytes.to_vec())
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02X}");
    }
    out
}

async fn matches_device_name(peripheral: &Peripheral) -> Result<bool> {
    for name in DEVICE_NAMES {
        if peripheral.local_name_matches(name).await? {
            return Ok(true);
        }
    }
    Ok(false)
}
