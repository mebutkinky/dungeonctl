//! Bluetooth LE protocol support for the DG-LAB PawPrints V1.1.

use std::ops::RangeInclusive;

use btleplug::{
    api::{
        Central, CentralEvent, CharPropFlags, Characteristic, Manager as _, Peripheral as _,
        WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use futures::{FutureExt, Stream, StreamExt, future::BoxFuture};
use tracing::debug;
use uuid::{Uuid, uuid};

use crate::{Error, Result, core::PeripheralExt};

const DEVICE_NAME: &str = "47L120300";
const WRITE_CHARACTERISTIC_UUID: Uuid = uuid!("0000150a-0000-1000-8000-00805f9b34fb");
const NOTIFY_CHARACTERISTIC_UUID: Uuid = uuid!("0000150b-0000-1000-8000-00805f9b34fb");

/// Implements the official V1.1 Bluetooth LE protocol.
#[derive(Debug)]
pub struct PawPrints {
    peripheral: Peripheral,
    write: Characteristic,
    write_type: WriteType,
}

bitflags::bitflags! {
    /// Bitflags representing the current state of the paw buttons.
    ///
    /// Multiple buttons may be active at the same time.
    /// A value of `0` indicates that no buttons are pressed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PawButtons: u8 {
        /// The bottom button is weakly pressed.
        const BOTTOM_WEAK = 0b0000_0001;
        /// The bottom button is strongly pressed.
        const BOTTOM_STRONG = 0b0000_0010;
        /// The top button is pressed.
        const TOP = 0b0000_0100;
    }
}

impl PawPrints {
    /// Begin building a connection to a PawPrints V1.1 device.
    pub fn connect() -> PawPrintsBuilder {
        PawPrintsBuilder::default()
    }

    /// Disconnect from the device.
    pub async fn disconnect(&self) -> Result<()> {
        self.peripheral.disconnect().await?;
        Ok(())
    }

    /// Validate and apply a complete `0x50` configuration.
    pub async fn update_settings(&self, settings: &Settings) -> Result<()> {
        self.send_raw(&settings.to_bytes()?).await
    }

    /// Reset the current mode's parameter values with command `0x5f`.
    pub async fn reset_parameters(&self) -> Result<()> {
        self.send_command(Command::ResetParameters).await
    }

    /// Start automatic XYZ angle-range detection with command `0x60`.
    pub async fn detect_angles(&self) -> Result<()> {
        self.send_command(Command::DetectAngles).await
    }

    /// Set a solid shoulder-light color with command `0x70`.
    pub async fn set_shoulder_color(&self, color: ShoulderColor) -> Result<()> {
        self.send_command(Command::ShoulderSolid(color)).await
    }

    /// Configure shoulder-light flashing with command `0x70`.
    pub async fn set_shoulder_flash(
        &self,
        first: ShoulderColor,
        second: ShoulderColor,
        speed: FlashSpeed,
    ) -> Result<()> {
        self.send_command(Command::ShoulderFlash {
            first,
            second,
            speed,
        })
        .await
    }

    /// Create a stream of parsed V1.1 notifications.
    pub async fn event_listener(&self) -> Result<impl Stream<Item = PawPrintsEvent> + Send> {
        let notifications = self.peripheral.notifications().await?;
        Ok(notifications.filter_map(|notification| async move {
            (notification.uuid == NOTIFY_CHARACTERISTIC_UUID)
                .then(|| PawPrintsEvent::from_bytes(&notification.value))
        }))
    }

    /// Run the notification loop and invoke an async callback for each event.
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

    async fn send_command(&self, command: Command) -> Result<()> {
        self.send_raw(&command.to_bytes()).await
    }

    async fn send_raw(&self, payload: &[u8]) -> Result<()> {
        debug!(payload = %hex_bytes(payload), "writing PawPrints V1.1 payload");
        self.peripheral
            .write(&self.write, payload, self.write_type)
            .await?;
        Ok(())
    }
}

/// Builder for a PawPrints V1.1 connection.
#[derive(Debug, Default)]
pub struct PawPrintsBuilder {
    adapter: Option<Adapter>,
    peripheral: Option<Peripheral>,
    settings: Settings,
}

impl PawPrintsBuilder {
    /// Use a specific Bluetooth adapter.
    pub fn with(mut self, adapter: impl Into<Adapter>) -> Self {
        self.adapter = Some(adapter.into());
        self
    }

    /// Connect to a specific peripheral instead of scanning by name.
    pub fn to(mut self, peripheral: impl Into<Peripheral>) -> Self {
        self.peripheral = Some(peripheral.into());
        self
    }

    /// Apply these settings immediately after connection.
    pub fn settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }

    async fn connect(self) -> Result<PawPrints> {
        let settings_bytes = self.settings.to_bytes()?;
        let adapter = match self.adapter {
            Some(adapter) => adapter,
            None => {
                let manager = Manager::new().await?;
                manager.adapters().await?.swap_remove(0)
            }
        };
        let peripheral = match self.peripheral {
            Some(peripheral) => peripheral,
            None => {
                adapter.start_scan(Default::default()).await?;
                let mut events = adapter.events().await?;
                let peripheral = loop {
                    if let Some(CentralEvent::DeviceDiscovered(id)) = events.next().await {
                        let candidate = adapter.peripheral(&id).await?;
                        if candidate.local_name_matches(DEVICE_NAME).await? {
                            break candidate;
                        }
                    }
                };
                adapter.stop_scan().await?;
                peripheral
            }
        };

        debug!(address = %peripheral.address(), "connecting to PawPrints V1.1");
        peripheral.connect().await?;
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
        let write_type = supported_write_type(write.properties);
        let pawprints = PawPrints {
            peripheral,
            write,
            write_type,
        };
        pawprints.send_raw(&settings_bytes).await?;
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

/// Main indicator colors accepted by the `0x50` command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MainColor {
    /// Yellow.
    Yellow = 1,
    /// Red.
    Red = 2,
    /// Purple.
    Purple = 3,
    /// Blue.
    Blue = 4,
    /// Cyan.
    Cyan = 5,
    /// Green.
    Green = 6,
}

impl MainColor {
    fn from_raw(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::Yellow,
            2 => Self::Red,
            3 => Self::Purple,
            4 => Self::Blue,
            5 => Self::Cyan,
            6 => Self::Green,
            _ => return None,
        })
    }
}

/// Shoulder-light colors accepted by the `0x70` command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ShoulderColor {
    /// Off.
    Off = 0,
    /// Yellow.
    Yellow = 1,
    /// Red.
    Red = 2,
    /// Purple.
    Purple = 3,
    /// Blue.
    Blue = 4,
    /// Cyan.
    Cyan = 5,
    /// Green.
    Green = 6,
    /// White.
    White = 7,
}

/// A valid PawPrints event identifier (`1..=24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventId(u8);

impl EventId {
    /// Validate and construct an event identifier.
    pub fn new(value: u8) -> Result<Self> {
        if (1..=24).contains(&value) {
            Ok(Self(value))
        } else {
            Err(Error::InvalidPawPrintsSettings(
                "event id must be in 1..=24",
            ))
        }
    }

    /// Return the raw identifier.
    pub fn get(self) -> u8 {
        self.0
    }
}

/// Complete V1.1 `0x50` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Main indicator color.
    pub main_color: MainColor,
    /// Trigger configuration.
    pub trigger: TriggerMode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::None,
        }
    }
}

/// A V1.1 trigger configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerMode {
    /// Disable all trigger behavior (`0x00`).
    None,
    /// Random-reaction mode (`0x03`).
    RandomReaction(RandomReaction),
    /// Probability mode (`0x04`).
    Probability {
        /// Six optional event/probability slots.
        events: [Option<ProbabilityEvent>; 6],
        /// Cooldown in seconds.
        cooldown: u16,
    },
    /// Combined button and motion mode (`0x05`).
    Combined(CombinedTrigger),
    /// External-voltage mode (`0x0f`).
    ExternalVoltage(ExternalVoltage),
    /// Physical-data stream (`0xd0`).
    PhysicalData,
}

/// Random-reaction settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RandomReaction {
    /// Event to emit on failure to react.
    pub event_id: EventId,
    /// Inclusive random delay bounds in seconds.
    pub random_delay: RangeInclusive<u16>,
    /// Reaction window in seconds.
    pub reaction_time: u16,
    /// Immediate parameter increase on trigger.
    pub trigger_increase: u8,
    /// Parameter increase speed during trigger.
    pub trigger_increase_speed: u8,
    /// Immediate parameter decrease on cancellation.
    pub cancel_decrease: u8,
    /// Parameter decrease speed after cancellation.
    pub cancel_decrease_speed: u8,
}

/// One probability-event slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbabilityEvent {
    /// Event identifier.
    pub event_id: EventId,
    /// Probability in half-percent units (`0..=200`).
    pub probability: u8,
}

/// Button and optional motion configuration sharing mode `0x05`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombinedTrigger {
    /// Optional button trigger.
    pub button: Option<ButtonTrigger>,
    /// Optional acceleration or angle trigger.
    pub motion: Option<MotionTrigger>,
}

/// Button edge settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonTrigger {
    /// Event identifier.
    pub event_id: EventId,
    /// Triggering edge.
    pub edge: ButtonEdge,
    /// Parameter increase speed during trigger.
    pub trigger_increase_speed: u8,
    /// Immediate parameter decrease on cancellation.
    pub cancel_decrease: u8,
    /// Parameter decrease speed after cancellation.
    pub cancel_decrease_speed: u8,
    /// Immediate parameter increase on trigger.
    pub trigger_increase: u8,
}

/// Button trigger edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEdge {
    /// Press edge.
    Press,
    /// Release edge.
    Release,
}

/// Threshold comparison direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThresholdComparison {
    /// Trigger below the threshold.
    Below,
    /// Trigger above the threshold.
    Above,
}

/// Inclusive signed angle interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AngleRange {
    /// Lower bound.
    pub lower: i8,
    /// Upper bound.
    pub upper: i8,
}

/// Motion portion of combined mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionTrigger {
    /// Overall acceleration threshold.
    Acceleration {
        /// Event identifier.
        event_id: EventId,
        /// Comparison direction.
        comparison: ThresholdComparison,
        /// Threshold (`0..=400`).
        threshold: u16,
        /// Trigger debounce (`0..=7`).
        trigger_debounce: u8,
        /// Cancellation debounce (`0..=7`).
        cancel_debounce: u8,
        /// Parameter mapping range.
        parameter_mapping: u8,
    },
    /// XYZ angle windows.
    Angle {
        /// Event identifier.
        event_id: EventId,
        /// X-axis window.
        x: AngleRange,
        /// Y-axis window.
        y: AngleRange,
        /// Z-axis window.
        z: AngleRange,
        /// Trigger debounce (`0..=7`).
        trigger_debounce: u8,
        /// Cancellation debounce (`0..=7`).
        cancel_debounce: u8,
        /// Parameter mapping range.
        parameter_mapping: u8,
    },
}

/// External input electrical mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoltageInputMode {
    /// Enable the documented internal pull-up network (`0`).
    InternalPullUp,
    /// High-impedance input (`1`).
    HighImpedance,
}

/// External-voltage trigger settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalVoltage {
    /// Event identifier.
    pub event_id: EventId,
    /// Input electrical mode.
    pub input_mode: VoltageInputMode,
    /// Lower target voltage in centivolts (`0..=210`).
    pub lower_threshold: u8,
    /// Upper target voltage in centivolts (`0..=210`).
    pub upper_threshold: u8,
    /// Parameter mapping voltage in centivolts (`0..=210`).
    pub parameter_mapping: u8,
}

impl Settings {
    /// Validate and serialize this `0x50` packet.
    pub fn to_bytes(&self) -> Result<[u8; 17]> {
        let mut bytes = [0; 17];
        bytes[0] = 0x50;
        bytes[1] = self.main_color as u8;
        match &self.trigger {
            TriggerMode::None => {}
            TriggerMode::RandomReaction(value) => {
                if value.random_delay.is_empty() {
                    return Err(Error::InvalidPawPrintsSettings(
                        "random delay lower bound must not exceed upper bound",
                    ));
                }
                if *value.random_delay.start() == 0 || value.reaction_time == 0 {
                    return Err(Error::InvalidPawPrintsSettings(
                        "random delay and reaction time must be at least 1 second",
                    ));
                }
                bytes[2] = 0x03;
                bytes[3] = value.event_id.get();
                bytes[4..6].copy_from_slice(&value.random_delay.start().to_be_bytes());
                bytes[6..8].copy_from_slice(&value.random_delay.end().to_be_bytes());
                bytes[8..10].copy_from_slice(&value.reaction_time.to_be_bytes());
                bytes[10] = value.trigger_increase;
                bytes[11] = value.trigger_increase_speed;
                bytes[12] = value.cancel_decrease;
                bytes[13] = value.cancel_decrease_speed;
            }
            TriggerMode::Probability { events, cooldown } => {
                let mut seen = [false; 25];
                let mut total = 0u16;
                bytes[2] = 0x04;
                for (index, item) in events.iter().enumerate() {
                    if let Some(item) = item {
                        let id = item.event_id.get() as usize;
                        if seen[id] {
                            return Err(Error::InvalidPawPrintsSettings(
                                "probability event ids must be unique",
                            ));
                        }
                        seen[id] = true;
                        total += u16::from(item.probability);
                        bytes[3 + index * 2] = id as u8;
                        bytes[4 + index * 2] = item.probability;
                    }
                }
                for index in (0..events.len()).rev() {
                    if total <= 200 {
                        break;
                    }
                    let probability_index = 4 + index * 2;
                    let reduction = (total - 200).min(u16::from(bytes[probability_index]));
                    bytes[probability_index] -= reduction as u8;
                    total -= reduction;
                }
                bytes[15..17].copy_from_slice(&cooldown.to_be_bytes());
            }
            TriggerMode::Combined(value) => {
                bytes[2] = 0x05;
                if let Some(button) = value.button {
                    bytes[3] = button.event_id.get();
                    bytes[5] = button.trigger_increase_speed;
                    bytes[6] = button.cancel_decrease;
                    bytes[7] = button.cancel_decrease_speed;
                    bytes[8] = button.trigger_increase;
                    if button.edge == ButtonEdge::Release {
                        bytes[4] |= 0x80;
                    }
                }
                if let Some(motion) = value.motion {
                    match motion {
                        MotionTrigger::Acceleration {
                            event_id,
                            comparison,
                            threshold,
                            trigger_debounce,
                            cancel_debounce,
                            parameter_mapping,
                        } => {
                            validate_debounce(trigger_debounce, cancel_debounce)?;
                            if threshold > 400 {
                                return Err(Error::InvalidPawPrintsSettings(
                                    "acceleration threshold must not exceed 400",
                                ));
                            }
                            bytes[9] = event_id.get();
                            bytes[10] = u8::from(comparison == ThresholdComparison::Above);
                            bytes[11..13].copy_from_slice(&threshold.to_be_bytes());
                            bytes[16] = parameter_mapping;
                            bytes[4] |= (trigger_debounce << 3) | cancel_debounce;
                        }
                        MotionTrigger::Angle {
                            event_id,
                            x,
                            y,
                            z,
                            trigger_debounce,
                            cancel_debounce,
                            parameter_mapping,
                        } => {
                            validate_debounce(trigger_debounce, cancel_debounce)?;
                            for range in [x, y, z] {
                                if range.lower > range.upper {
                                    return Err(Error::InvalidPawPrintsSettings(
                                        "angle lower bound must not exceed upper bound",
                                    ));
                                }
                            }
                            bytes[9] = event_id.get();
                            bytes[10..16].copy_from_slice(&[
                                x.lower as u8,
                                x.upper as u8,
                                y.lower as u8,
                                y.upper as u8,
                                z.lower as u8,
                                z.upper as u8,
                            ]);
                            bytes[16] = parameter_mapping;
                            bytes[4] |= 0x40 | (trigger_debounce << 3) | cancel_debounce;
                        }
                    }
                }
            }
            TriggerMode::ExternalVoltage(value) => {
                if value.lower_threshold > 210
                    || value.upper_threshold > 210
                    || value.parameter_mapping > 210
                {
                    return Err(Error::InvalidPawPrintsSettings(
                        "voltage values must not exceed 210",
                    ));
                }
                if value.lower_threshold > value.upper_threshold {
                    return Err(Error::InvalidPawPrintsSettings(
                        "voltage lower bound must not exceed upper bound",
                    ));
                }
                bytes[2] = 0x0f;
                bytes[3] = value.event_id.get();
                bytes[4] = u8::from(value.input_mode == VoltageInputMode::HighImpedance);
                bytes[5] = value.lower_threshold;
                bytes[6] = value.upper_threshold;
                bytes[7] = value.parameter_mapping;
            }
            TriggerMode::PhysicalData => bytes[2] = 0xd0,
        }
        Ok(bytes)
    }
}

fn validate_debounce(trigger: u8, cancel: u8) -> Result<()> {
    if trigger <= 7 && cancel <= 7 {
        Ok(())
    } else {
        Err(Error::InvalidPawPrintsSettings(
            "debounce must not exceed 7",
        ))
    }
}

/// Standalone V1.1 commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Reset current parameter values (`0x5f`).
    ResetParameters,
    /// Begin automatic angle detection (`0x60`).
    DetectAngles,
    /// Set a solid shoulder-light color.
    ShoulderSolid(ShoulderColor),
    /// Configure shoulder-light flashing.
    ShoulderFlash {
        /// First color.
        first: ShoulderColor,
        /// Second color.
        second: ShoulderColor,
        /// Flash speed/action.
        speed: FlashSpeed,
    },
}

impl Command {
    /// Serialize the command.
    pub fn to_bytes(self) -> Vec<u8> {
        match self {
            Self::ResetParameters => vec![0x5f],
            Self::DetectAngles => vec![0x60],
            Self::ShoulderSolid(color) => vec![0x70, color as u8],
            Self::ShoulderFlash {
                first,
                second,
                speed,
            } => vec![0x70, first as u8, second as u8, speed as u8],
        }
    }
}

/// Shoulder-light flash speed or stop action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FlashSpeed {
    /// Slow flashing.
    Slow = 1,
    /// Fast flashing.
    Fast = 2,
    /// Stop flashing.
    Stop = 3,
}

/// One signed range reported by automatic angle detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetectedRange {
    /// Raw signed lower bound.
    pub lower: i16,
    /// Raw signed upper bound.
    pub upper: i16,
}

/// Calibrated acceleration values for the three sensor axes.
///
/// Values are stored as floating-point numbers to allow conversion into
/// physical units and derived calculations such as tilt or magnitude.
#[derive(Debug, Clone, Copy, Default)]
pub struct Accel {
    /// Acceleration along the X axis.
    pub x: f32,
    /// Acceleration along the Y axis.
    pub y: f32,
    /// Acceleration along the Z axis.
    pub z: f32,
}

/// Raw acceleration readings received directly from the sensor.
///
/// Each axis is stored as a signed 8-bit value before any scaling,
/// calibration, or unit conversion is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawAccel {
    /// Raw reading for the X axis.
    pub x: i8,
    /// Raw reading for the Y axis.
    pub y: i8,
    /// Raw reading for the Z axis.
    pub z: i8,
}

impl From<RawAccel> for Accel {
    fn from(raw: RawAccel) -> Self {
        Self {
            x: raw.x as f32,
            y: raw.y as f32,
            z: raw.z as f32,
        }
    }
}

impl RawAccel {
    /// convert to usable type
    pub fn to_accel(self) -> Accel {
        Accel::from(self)
    }

    fn from_u8(x: u8, y: u8, z: u8) -> Self {
        Self {
            x: x as i8,
            y: y as i8,
            z: z as i8,
        }
    }
}
/// This is typically used to describe which side of the sensor is pointing

/// most strongly in the direction of acceleration, such as gravity.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub enum AxisDirection {
    /// Positive X direction.
    PosX,
    /// Negative X direction.
    NegX,
    /// Positive Y direction.
    PosY,
    /// Negative Y direction.
    NegY,
    /// Positive Z direction.
    PosZ,
    /// Negative Z direction.
    NegZ,
}
impl Accel {
    /// Total acceleration vector magnitude.
    pub fn magnitude(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    /// Returns the vector normalized to length 1.
    pub fn normalized(&self) -> Self {
        let mag = self.magnitude();

        if mag == 0.0 {
            return *self;
        }

        Self {
            x: self.x / mag,
            y: self.y / mag,
            z: self.z / mag,
        }
    }

    /// Roll angle in radians.
    pub fn roll(&self) -> f32 {
        self.y.atan2(self.z)
    }

    /// Pitch angle in radians.
    pub fn pitch(&self) -> f32 {
        (-self.x).atan2((self.y * self.y + self.z * self.z).sqrt())
    }

    /// Roll angle in degrees.
    pub fn roll_degrees(&self) -> f32 {
        self.roll().to_degrees()
    }

    /// Pitch angle in degrees.
    pub fn pitch_degrees(&self) -> f32 {
        self.pitch().to_degrees()
    }

    /// Which axis currently carries the largest acceleration component.
    pub fn dominant_axis(&self) -> AxisDirection {
        let ax = self.x.abs();
        let ay = self.y.abs();
        let az = self.z.abs();

        if ax >= ay && ax >= az {
            if self.x >= 0.0 {
                AxisDirection::PosX
            } else {
                AxisDirection::NegX
            }
        } else if ay >= az {
            if self.y >= 0.0 {
                AxisDirection::PosY
            } else {
                AxisDirection::NegY
            }
        } else if self.z >= 0.0 {
            AxisDirection::PosZ
        } else {
            AxisDirection::NegZ
        }
    }

    /// Difference between this reading and another reading.
    pub fn delta(&self, other: &Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }

    /// Angle between two acceleration vectors, in radians.
    pub fn angle_to(&self, other: &Self) -> f32 {
        let dot = self.x * other.x + self.y * other.y + self.z * other.z;

        let mags = self.magnitude() * other.magnitude();

        if mags == 0.0 {
            return 0.0;
        }

        (dot / mags).clamp(-1.0, 1.0).acos()
    }

    /// Angle between two acceleration vectors, in degrees.
    pub fn angle_to_degrees(&self, other: &Self) -> f32 {
        self.angle_to(other).to_degrees()
    }
}

/// A parsed V1.1 notification.
#[derive(Clone, Debug, PartialEq)]
pub enum PawPrintsEvent {
    /// `0x51` status and battery report.
    Status {
        /// Main indicator color.
        main_color: MainColor,
        /// Device type (`3` for V1.1).
        device_type: u8,
        /// Battery level.
        battery: u8,
    },
    /// `0x5a` event-triggered report.
    Triggered {
        /// Main indicator color.
        main_color: MainColor,
        /// Triggered event.
        event_id: EventId,
        /// Current parameter.
        parameter: u8,
    },
    /// `0x5b` event-canceled report.
    Canceled {
        /// Main indicator color.
        main_color: MainColor,
        /// Canceled event.
        event_id: EventId,
    },
    /// `0x5c` parameter-change report.
    ParameterChanged {
        /// Main indicator color.
        main_color: MainColor,
        /// Changed event.
        event_id: EventId,
        /// New parameter.
        parameter: u8,
    },
    /// `0xd0` physical sensor report.
    PhysicalData {
        /// Main indicator color.
        main_color: MainColor,
        /// Packet sequence number.
        sequence: u8,
        /// Whether the button is pressed.
        pressed: PawButtons,
        /// Overall acceleration sample.
        acceleration: u8,
        /// acceleration
        accel: RawAccel,
        /// External-voltage sample in centivolts.
        external_voltage: u8,
    },
    /// `0xf1 0x61` automatic XYZ detection result.
    DetectedAngles {
        /// X range.
        x: DetectedRange,
        /// Y range.
        y: DetectedRange,
        /// Z range.
        z: DetectedRange,
    },
    /// An unrecognized or malformed packet.
    Unknown(Vec<u8>),
}

impl PawPrintsEvent {
    /// Parse one notification, retaining malformed packets as [`Self::Unknown`].
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let color = || bytes.get(1).copied().and_then(MainColor::from_raw);
        let event_id = |index| {
            bytes
                .get(index)
                .copied()
                .and_then(|value| EventId::new(value).ok())
        };
        match bytes {
            [0x51, _, 0x03, battery] if color().is_some() => Self::Status {
                main_color: color().unwrap(),
                device_type: 0x03,
                battery: *battery,
            },
            [0x5a, _, _, parameter] if color().is_some() && event_id(2).is_some() => {
                Self::Triggered {
                    main_color: color().unwrap(),
                    event_id: event_id(2).unwrap(),
                    parameter: *parameter,
                }
            }
            [0x5b, _, _] if color().is_some() && event_id(2).is_some() => Self::Canceled {
                main_color: color().unwrap(),
                event_id: event_id(2).unwrap(),
            },
            [0x5c, _, _, parameter] if color().is_some() && event_id(2).is_some() => {
                Self::ParameterChanged {
                    main_color: color().unwrap(),
                    event_id: event_id(2).unwrap(),
                    parameter: *parameter,
                }
            }
            [
                0xd0,
                _,
                sequence,
                pressed,
                acceleration,
                x,
                y,
                z,
                external_voltage,
            ] if color().is_some() => Self::PhysicalData {
                main_color: color().unwrap(),
                sequence: *sequence,
                pressed: PawButtons::from_bits_truncate(*pressed),
                acceleration: *acceleration,
                accel: RawAccel::from_u8(*x, *y, *z),
                external_voltage: *external_voltage,
            },
            [0xf1, 0x61, rest @ ..] if rest.len() == 12 => Self::DetectedAngles {
                x: detected_range(&rest[0..4]),
                y: detected_range(&rest[4..8]),
                z: detected_range(&rest[8..12]),
            },
            _ => Self::Unknown(bytes.to_vec()),
        }
    }

    /// Convert an automatic detection result into V1.1 angle-trigger ranges.
    pub fn angle_ranges(&self) -> Option<[AngleRange; 3]> {
        let Self::DetectedAngles { x, y, z } = self else {
            return None;
        };
        Some([convert_range(*x), convert_range(*y), convert_range(*z)])
    }
}

fn detected_range(bytes: &[u8]) -> DetectedRange {
    DetectedRange {
        lower: i16::from_be_bytes([bytes[0], bytes[1]]),
        upper: i16::from_be_bytes([bytes[2], bytes[3]]),
    }
}

fn convert_range(range: DetectedRange) -> AngleRange {
    AngleRange {
        lower: (range.lower / 2).clamp(-128, 127) as i8,
        upper: (range.upper / 2).clamp(-128, 127) as i8,
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02X}");
    }
    output
}

fn supported_write_type(properties: CharPropFlags) -> WriteType {
    if properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        WriteType::WithoutResponse
    } else {
        WriteType::WithResponse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use btleplug::api::CharPropFlags;
    use hex_literal::hex;

    fn event(id: u8) -> EventId {
        EventId::new(id).unwrap()
    }

    #[test]
    fn serializes_official_random_reaction_example() {
        let settings = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::RandomReaction(RandomReaction {
                event_id: event(7),
                random_delay: 30..=50,
                reaction_time: 10,
                trigger_increase: 20,
                trigger_increase_speed: 20,
                cancel_decrease: 50,
                cancel_decrease_speed: 20,
            }),
        };

        assert_eq!(
            settings.to_bytes().unwrap(),
            hex!("50010307001E0032000A14143214000000")
        );
    }

    #[test]
    fn serializes_official_probability_example() {
        let settings = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::Probability {
                events: [
                    Some(ProbabilityEvent {
                        event_id: event(5),
                        probability: 60,
                    }),
                    Some(ProbabilityEvent {
                        event_id: event(2),
                        probability: 80,
                    }),
                    None,
                    None,
                    None,
                    None,
                ],
                cooldown: 600,
            },
        };

        assert_eq!(
            settings.to_bytes().unwrap(),
            hex!("500104053C025000000000000000000258")
        );
    }

    #[test]
    fn serializes_official_external_voltage_example() {
        let settings = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::ExternalVoltage(ExternalVoltage {
                event_id: event(7),
                input_mode: VoltageInputMode::InternalPullUp,
                lower_threshold: 13,
                upper_threshold: 18,
                parameter_mapping: 8,
            }),
        };

        assert_eq!(
            settings.to_bytes().unwrap(),
            hex!("50010F07000D1208000000000000000000")
        );
    }

    #[test]
    fn serializes_official_combined_button_and_acceleration_example() {
        let settings = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::Combined(CombinedTrigger {
                button: Some(ButtonTrigger {
                    event_id: event(20),
                    edge: ButtonEdge::Release,
                    trigger_increase_speed: 20,
                    cancel_decrease: 50,
                    cancel_decrease_speed: 20,
                    trigger_increase: 20,
                }),
                motion: Some(MotionTrigger::Acceleration {
                    event_id: event(10),
                    comparison: ThresholdComparison::Above,
                    threshold: 100,
                    trigger_debounce: 3,
                    cancel_debounce: 6,
                    parameter_mapping: 100,
                }),
            }),
        };

        assert_eq!(
            settings.to_bytes().unwrap(),
            hex!("500105149E143214140A01006400000064")
        );
    }

    #[test]
    fn rejects_invalid_probability_settings() {
        let duplicate = ProbabilityEvent {
            event_id: event(1),
            probability: 100,
        };
        let settings = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::Probability {
                events: [Some(duplicate), Some(duplicate), None, None, None, None],
                cooldown: 0,
            },
        };
        assert!(settings.to_bytes().is_err());
    }

    #[test]
    fn serializes_none_physical_data_and_official_angle_example() {
        assert_eq!(
            Settings::default().to_bytes().unwrap(),
            hex!("5001000000000000000000000000000000")
        );
        assert_eq!(
            Settings {
                main_color: MainColor::Blue,
                trigger: TriggerMode::PhysicalData,
            }
            .to_bytes()
            .unwrap(),
            hex!("5004D00000000000000000000000000000")
        );
        let angle = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::Combined(CombinedTrigger {
                button: None,
                motion: Some(MotionTrigger::Angle {
                    event_id: event(5),
                    x: AngleRange {
                        lower: 10,
                        upper: 50,
                    },
                    y: AngleRange {
                        lower: 20,
                        upper: 30,
                    },
                    z: AngleRange {
                        lower: 40,
                        upper: 60,
                    },
                    trigger_debounce: 3,
                    cancel_debounce: 6,
                    parameter_mapping: 100,
                }),
            }),
        };
        assert_eq!(
            angle.to_bytes().unwrap(),
            hex!("500105005E00000000050A32141E283C64")
        );
    }

    #[test]
    fn rejects_out_of_range_settings() {
        assert!(EventId::new(0).is_err());
        assert!(EventId::new(25).is_err());

        let random = |delay, speed| Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::RandomReaction(RandomReaction {
                event_id: event(1),
                random_delay: delay,
                reaction_time: 1,
                trigger_increase: 0,
                trigger_increase_speed: speed,
                cancel_decrease: 0,
                cancel_decrease_speed: 0,
            }),
        };
        let lower = 2;
        let upper = 1;
        assert!(random(lower..=upper, 0).to_bytes().is_err());
        assert!(random(0..=2, 0).to_bytes().is_err());
        let zero_reaction_time = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::RandomReaction(RandomReaction {
                event_id: event(1),
                random_delay: 1..=2,
                reaction_time: 0,
                trigger_increase: 0,
                trigger_increase_speed: 0,
                cancel_decrease: 0,
                cancel_decrease_speed: 0,
            }),
        };
        assert!(zero_reaction_time.to_bytes().is_err());

        let acceleration = |threshold, debounce| Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::Combined(CombinedTrigger {
                button: None,
                motion: Some(MotionTrigger::Acceleration {
                    event_id: event(1),
                    comparison: ThresholdComparison::Below,
                    threshold,
                    trigger_debounce: debounce,
                    cancel_debounce: 0,
                    parameter_mapping: 0,
                }),
            }),
        };
        assert!(acceleration(401, 0).to_bytes().is_err());
        assert!(acceleration(400, 8).to_bytes().is_err());

        let voltage = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::ExternalVoltage(ExternalVoltage {
                event_id: event(1),
                input_mode: VoltageInputMode::HighImpedance,
                lower_threshold: 0,
                upper_threshold: 211,
                parameter_mapping: 0,
            }),
        };
        assert!(voltage.to_bytes().is_err());
    }

    #[test]
    fn serializes_standalone_commands() {
        assert_eq!(Command::ResetParameters.to_bytes(), vec![0x5f]);
        assert_eq!(Command::DetectAngles.to_bytes(), vec![0x60]);
        assert_eq!(
            Command::ShoulderSolid(ShoulderColor::Blue).to_bytes(),
            vec![0x70, 0x04]
        );
        assert_eq!(
            Command::ShoulderFlash {
                first: ShoulderColor::Red,
                second: ShoulderColor::White,
                speed: FlashSpeed::Fast,
            }
            .to_bytes(),
            vec![0x70, 0x02, 0x07, 0x02],
        );
    }

    #[test]
    fn parses_all_documented_notifications() {
        assert_eq!(
            PawPrintsEvent::from_bytes(&[0x51, 0x04, 0x03, 87]),
            PawPrintsEvent::Status {
                main_color: MainColor::Blue,
                device_type: 3,
                battery: 87
            },
        );
        assert_eq!(
            PawPrintsEvent::from_bytes(&[0x5a, 0x02, 24, 255]),
            PawPrintsEvent::Triggered {
                main_color: MainColor::Red,
                event_id: event(24),
                parameter: 255
            },
        );
        assert_eq!(
            PawPrintsEvent::from_bytes(&[0x5b, 0x03, 7]),
            PawPrintsEvent::Canceled {
                main_color: MainColor::Purple,
                event_id: event(7)
            },
        );
        assert_eq!(
            PawPrintsEvent::from_bytes(&[0x5c, 0x06, 1, 99]),
            PawPrintsEvent::ParameterChanged {
                main_color: MainColor::Green,
                event_id: event(1),
                parameter: 99
            },
        );
        assert_eq!(
            PawPrintsEvent::from_bytes(&[0xd0, 0x01, 9, 1, 200, 0xff, 0x80, 0x7f, 210]),
            PawPrintsEvent::PhysicalData {
                main_color: MainColor::Yellow,
                sequence: 9,
                pressed: PawButtons::BOTTOM_WEAK,
                acceleration: 200,
                accel: RawAccel {
                    x: -1,
                    y: -128,
                    z: 127,
                },
                external_voltage: 210,
            },
        );
        assert_eq!(
            PawPrintsEvent::from_bytes(&hex!("F161FE0001FF000200FFFE000300")),
            PawPrintsEvent::DetectedAngles {
                x: DetectedRange {
                    lower: -512,
                    upper: 511
                },
                y: DetectedRange {
                    lower: 2,
                    upper: 255
                },
                z: DetectedRange {
                    lower: -512,
                    upper: 768
                },
            },
        );
    }

    #[test]
    fn converts_detected_angles_for_trigger_settings() {
        let event = PawPrintsEvent::DetectedAngles {
            x: DetectedRange {
                lower: -512,
                upper: 511,
            },
            y: DetectedRange {
                lower: -3,
                upper: 3,
            },
            z: DetectedRange {
                lower: -256,
                upper: 254,
            },
        };
        assert_eq!(
            event.angle_ranges(),
            Some([
                AngleRange {
                    lower: -128,
                    upper: 127
                },
                AngleRange {
                    lower: -1,
                    upper: 1
                },
                AngleRange {
                    lower: -128,
                    upper: 127
                },
            ]),
        );
    }

    #[test]
    fn leaves_malformed_notifications_unknown() {
        for packet in [
            vec![0x51, 0x07, 0x03, 80],
            vec![0x5a, 0x01, 0, 1],
            vec![0x5b, 0x01, 1, 0],
            vec![0xd0; 8],
            vec![0xf1, 0x60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ] {
            assert_eq!(
                PawPrintsEvent::from_bytes(&packet),
                PawPrintsEvent::Unknown(packet)
            );
        }
    }

    #[test]
    fn selects_a_supported_ble_write_type() {
        assert_eq!(
            supported_write_type(CharPropFlags::WRITE_WITHOUT_RESPONSE),
            WriteType::WithoutResponse
        );
        assert_eq!(
            supported_write_type(CharPropFlags::WRITE),
            WriteType::WithResponse
        );
    }

    #[test]
    fn matches_documented_edge_behavior() {
        let probabilities = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::Probability {
                events: [
                    Some(ProbabilityEvent {
                        event_id: event(1),
                        probability: 20,
                    }),
                    Some(ProbabilityEvent {
                        event_id: event(8),
                        probability: 30,
                    }),
                    Some(ProbabilityEvent {
                        event_id: event(11),
                        probability: 40,
                    }),
                    Some(ProbabilityEvent {
                        event_id: event(4),
                        probability: 50,
                    }),
                    Some(ProbabilityEvent {
                        event_id: event(5),
                        probability: 70,
                    }),
                    Some(ProbabilityEvent {
                        event_id: event(6),
                        probability: 70,
                    }),
                ],
                cooldown: 600,
            },
        };
        assert_eq!(
            probabilities.to_bytes().unwrap(),
            hex!("5001040114081E0B280432053C06000258")
        );

        let high_speeds = Settings {
            main_color: MainColor::Yellow,
            trigger: TriggerMode::Combined(CombinedTrigger {
                button: Some(ButtonTrigger {
                    event_id: event(1),
                    edge: ButtonEdge::Press,
                    trigger_increase_speed: 121,
                    cancel_decrease: 0,
                    cancel_decrease_speed: 255,
                    trigger_increase: 0,
                }),
                motion: None,
            }),
        };
        assert!(high_speeds.to_bytes().is_ok());

        let packet = vec![0x51, 0x01, 0x02, 80];
        assert_eq!(
            PawPrintsEvent::from_bytes(&packet),
            PawPrintsEvent::Unknown(packet)
        );
    }
}
