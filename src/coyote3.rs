//! Implemention of the Bluetooth LE protocols to control the DG-LAB Coyote 3.

use std::{
    ops::Deref,
    sync::{Arc, Mutex},
};

use arrayvec::ArrayVec;
use btleplug::{
    api::{Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, WriteType},
    platform::{Adapter, Manager, Peripheral},
};
use futures::{FutureExt, StreamExt, future::BoxFuture};
use smart_default::SmartDefault;
use tokio::task::JoinHandle;
use tracing::{debug, error};
use uuid::{Uuid, uuid};

mod runtime;
mod waveforms;

pub use self::runtime::{
    DoNotChangeIntensityShape, EncodedShape, EncodedShapeContext, IntensityContext, IntensityShape,
    Runtime, Shape, ShapeContext, ZeroShape,
};
pub use self::waveforms::{Breathing, Tidal};

use crate::{
    Error, LedColor, Result,
    core::{DeviceState, PeripheralExt, StatePublisher, StateSignal, Stereo},
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
    state_publisher: StatePublisher<State>,
    intensity_sequencer: Arc<Mutex<IntensitySequencer>>,
    notification_task: Mutex<Option<JoinHandle<()>>>,
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

    /// Create a pulse runtime for this Coyote 3.
    pub fn runtime(self) -> Runtime {
        Runtime::new(self)
    }

    /// Disconnect from the Coyote3.
    pub async fn disconnect(&self) -> Result<()> {
        if let Some(task) = self.notification_task.lock().unwrap().take() {
            task.abort();
        }
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

        let settings = self.settings.normalized();

        debug!("connecting to {}", peripheral.address());
        peripheral.connect().await?;
        debug!("discovering services");
        peripheral.discover_services().await?;

        let characteristics = peripheral.characteristics();
        validate_characteristic_uuids(characteristics.iter().map(|item| item.uuid))?;
        let battery = characteristics
            .iter()
            .find(|item| item.uuid == BATTERY_CHARACTERISTIC_UUID)
            .expect("validated battery characteristic")
            .clone();
        let notify = characteristics
            .iter()
            .find(|item| item.uuid == NOTIFY_CHARACTERISTIC_UUID)
            .expect("validated notify characteristic")
            .clone();
        let write = characteristics
            .iter()
            .find(|item| item.uuid == WRITE_CHARACTERISTIC_UUID)
            .expect("validated write characteristic")
            .clone();

        peripheral.subscribe(&battery).await?;
        peripheral.subscribe(&notify).await?;

        let battery_value = peripheral.read(&battery).await?;
        let battery_level = parse_battery_payload(&battery_value).unwrap_or_else(|| {
            error!(value = ?battery_value, "invalid Coyote battery payload");
            0
        });
        let initial_state = State {
            battery: battery_level,
            settings,
            intensity: Stereo { a: 0, b: 0 },
            last_intensity_sequence: 0,
            intensity_update_pending: false,
        };

        let (state, state_publisher) = DeviceState::channel(initial_state);
        let intensity_sequencer = Arc::new(Mutex::new(IntensitySequencer::default()));
        let mut notifications = peripheral.notifications().await?;
        let notification_state = state_publisher.clone();
        let notification_sequencer = Arc::clone(&intensity_sequencer);
        let notification_task = tokio::spawn(async move {
            while let Some(notification) = notifications.next().await {
                debug!(?notification);
                match notification.uuid {
                    NOTIFY_CHARACTERISTIC_UUID => match parse_notification(&notification.value) {
                        Some(notification) => {
                            let mut sequencer = notification_sequencer.lock().unwrap();
                            notification_state.update(|state| {
                                apply_protocol_notification(state, &mut sequencer, notification);
                            });
                        }
                        None => {
                            error!(value = ?notification.value, "invalid Coyote notification");
                        }
                    },
                    BATTERY_CHARACTERISTIC_UUID => {
                        if let Some(battery) = parse_battery_payload(&notification.value) {
                            notification_state.update(|state| state.battery = battery);
                        } else {
                            error!(value = ?notification.value, "invalid Coyote battery payload");
                        }
                    }
                    uuid => {
                        debug!("received notification for unknown characteristic {uuid}");
                    }
                }
            }
        });

        let coyote = Coyote3 {
            peripheral: peripheral.clone(),
            write,
            state,
            state_publisher,
            intensity_sequencer,
            notification_task: Mutex::new(Some(notification_task)),
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
    /// Active frequencies above the documented 100 Hz maximum are encoded as 100 Hz.
    /// A frequency of zero disables that channel's waveform for the frame.
    ///
    /// Intensity changes use the documented B0/B1 sequence handshake. If a change is still
    /// awaiting acknowledgement, waveform data continues immediately while later intensity
    /// changes are coalesced for the next acknowledged command.
    pub async fn send_pulses(&self, pulses: Pulses) -> Result<()> {
        self.send_wire_pulses(pulses.intensity, Pulses::convert_pulses(&pulses.pulses))
            .await
    }

    async fn send_encoded_pulses(&self, pulses: EncodedPulses) -> Result<()> {
        self.send_wire_pulses(
            pulses.intensity,
            EncodedPulses::convert_pulses(&pulses.pulses),
        )
        .await
    }

    async fn send_wire_pulses(
        &self,
        intensity: Stereo<IntensityChange>,
        pulses: [[u8; 4]; 4],
    ) -> Result<()> {
        let prepared = self.intensity_sequencer.lock().unwrap().prepare(intensity);
        let waiting = self.intensity_sequencer.lock().unwrap().is_waiting();
        self.state_publisher
            .update(|state| state.intensity_update_pending = waiting);
        let frame = B0Frame {
            intensity: prepared,
            pulses,
        };

        if let Err(error) = self.send_command(Command::SendPulses(frame)).await {
            let waiting = {
                let mut sequencer = self.intensity_sequencer.lock().unwrap();
                sequencer.rollback(prepared);
                sequencer.is_waiting()
            };
            self.state_publisher
                .update(|state| state.intensity_update_pending = waiting);
            return Err(error);
        }

        Ok(())
    }
    /// Update the device settings.
    ///
    /// Soft intensity limits are constrained to the documented `0..=200` range. Both balance
    /// parameters retain their full-byte `0..=255` range. The BF command has no response and its
    /// settings persist on the device, so these settings are sent after every connection.
    pub async fn update_settings(&self, settings: DeviceSettings) -> Result<()> {
        let settings = settings.normalized();
        self.send_command(Command::UpdateSettings(settings)).await?;
        self.state_publisher
            .update(|state| state.settings = settings);
        Ok(())
    }

    /// sets the color for eye indicator(todo)
    pub async fn set_color(&self, color: LedColor) -> Result<()> {
        let mut payload = [0u8; 17];
        payload[0] = 0x50;
        payload[1] = color.raw();

        self.peripheral
            .write(&self.write, &payload, WriteType::WithoutResponse)
            .await?;
        Ok(())
    }

    /// sets the color for battery indicator
    pub async fn set_led_color(&self, color: LedColor) -> Result<()> {
        self.peripheral
            .write(
                &self.write,
                &[0x70, color.raw()],
                WriteType::WithoutResponse,
            )
            .await?;
        Ok(())
    }

    async fn send_command(&self, command: Command) -> Result<()> {
        debug!(?command);
        self.peripheral
            .write(&self.write, &command.to_bytes(), WriteType::WithoutResponse)
            .await?;

        Ok(())
    }
}

impl Drop for Coyote3 {
    fn drop(&mut self) {
        if let Some(task) = self.notification_task.lock().unwrap().take() {
            task.abort();
        }
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
    /// The sequence number from the most recently received B1 notification.
    pub last_intensity_sequence: u8,
    /// Whether an intensity-changing B0 command is awaiting its matching B1 response.
    pub intensity_update_pending: bool,
}

/// The device settings of the Coyote 3.
#[derive(Clone, Copy, Debug, PartialEq, SmartDefault, binrw::BinRead, binrw::BinWrite)]
#[brw(big)]
pub struct DeviceSettings {
    /// The maximum intensity limit, constrained to `0..=200` when encoded.
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

impl DeviceSettings {
    fn normalized(mut self) -> Self {
        self.limit.a = self.limit.a.min(200);
        self.limit.b = self.limit.b.min(200);
        self
    }
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct EncodedPulses {
    pub(crate) intensity: Stereo<IntensityChange>,
    pub(crate) pulses: [Stereo<EncodedPulse>; 4],
}

impl Pulses {
    fn convert_pulses(pulses: &[Stereo<Pulse>; 4]) -> [[u8; 4]; 4] {
        [
            pulses.map(|p| p.a.encoded_frequency()),
            pulses.map(|p| p.a.clamped_intensity()),
            pulses.map(|p| p.b.encoded_frequency()),
            pulses.map(|p| p.b.clamped_intensity()),
        ]
    }
}

impl EncodedPulses {
    fn convert_pulses(pulses: &[Stereo<EncodedPulse>; 4]) -> [[u8; 4]; 4] {
        [
            pulses.map(|pulse| pulse.a.normalized().0),
            pulses.map(|pulse| pulse.a.normalized().1),
            pulses.map(|pulse| pulse.b.normalized().0),
            pulses.map(|pulse| pulse.b.normalized().1),
        ]
    }
}

/// A single frequency-intensity set representing 25 ms of a waveform for a single channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pulse {
    /// The frequency in Hz. Active values are constrained to the documented `1..=100` range;
    /// zero disables the channel.
    pub frequency: u8,
    /// The pulse amplitude as an abstract value in the range of 0 to 100.
    pub intensity: u8,
}

impl Pulse {
    pub(crate) fn encoded_frequency(&self) -> u8 {
        if self.frequency == 0 {
            return 0;
        }

        let frequency = self.frequency.min(100);
        let t = 1000.0 / (frequency as f32);

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
    pub(crate) fn clamped_intensity(&self) -> u8 {
        self.intensity.clamp(0, 100)
    }
}

/// A protocol-native frequency-intensity pair for one 25 ms waveform chunk.
///
/// Unlike [`Pulse`], `frequency` is the compressed V3 protocol value rather
/// than a value in hertz. Active values are normalized to `10..=240`; zero
/// disables the channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodedPulse {
    /// The compressed V3 frequency value, or zero to disable the channel.
    pub frequency: u8,
    /// The relative waveform intensity in the range `0..=100`.
    pub intensity: u8,
}

impl EncodedPulse {
    pub(crate) fn normalized(self) -> (u8, u8) {
        let frequency = if self.frequency == 0 {
            0
        } else {
            self.frequency.clamp(10, 240)
        };

        (frequency, self.intensity.min(100))
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
    /// Increase the intensity by `x`, constrained to `0..=200` when encoded.
    RelativeIncrease(u8),
    /// Decrease the intensity by `x`, constrained to `0..=200` when encoded.
    RelativeDecrease(u8),
    /// Set the intensity to `x`, constrained to `0..=200` when encoded.
    AbsoluteChange(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PendingIntensityChange {
    #[default]
    None,
    Relative(i16),
    Absolute(i16),
}

impl PendingIntensityChange {
    fn then(self, change: IntensityChange) -> Self {
        match change {
            IntensityChange::DoNotChange => self,
            IntensityChange::AbsoluteChange(value) => Self::Absolute(i16::from(value.min(200))),
            IntensityChange::RelativeIncrease(value) => self.then_delta(i16::from(value.min(200))),
            IntensityChange::RelativeDecrease(value) => self.then_delta(-i16::from(value.min(200))),
        }
    }

    fn then_delta(self, delta: i16) -> Self {
        match self {
            Self::None | Self::Relative(0) => Self::Relative(delta.clamp(-200, 200)),
            Self::Relative(value) => Self::Relative((value + delta).clamp(-200, 200)),
            Self::Absolute(value) => Self::Absolute((value + delta).clamp(0, 200)),
        }
    }

    fn into_change(self) -> IntensityChange {
        match self {
            Self::None | Self::Relative(0) => IntensityChange::DoNotChange,
            Self::Relative(value) if value > 0 => IntensityChange::RelativeIncrease(value as u8),
            Self::Relative(value) => IntensityChange::RelativeDecrease((-value) as u8),
            Self::Absolute(value) => IntensityChange::AbsoluteChange(value as u8),
        }
    }

    fn prepend(self, change: IntensityChange) -> Self {
        PendingIntensityChange::default().then(change).compose(self)
    }

    fn compose(self, later: Self) -> Self {
        match later {
            Self::None => self,
            Self::Relative(delta) => self.then_delta(delta),
            Self::Absolute(value) => Self::Absolute(value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreparedIntensity {
    sequence: u8,
    intensity: Stereo<IntensityChange>,
}

#[derive(Debug)]
struct IntensitySequencer {
    next_sequence: u8,
    outstanding: Option<PreparedIntensity>,
    pending: Stereo<PendingIntensityChange>,
}

impl Default for IntensitySequencer {
    fn default() -> Self {
        Self {
            next_sequence: 1,
            outstanding: None,
            pending: Stereo::symmetric(PendingIntensityChange::None),
        }
    }
}

impl IntensitySequencer {
    fn prepare(&mut self, intensity: Stereo<IntensityChange>) -> PreparedIntensity {
        if let Some(outstanding) = self.outstanding {
            self.pending.a =
                Self::queue_after_outstanding(self.pending.a, outstanding.intensity.a, intensity.a);
            self.pending.b =
                Self::queue_after_outstanding(self.pending.b, outstanding.intensity.b, intensity.b);
            return PreparedIntensity {
                sequence: 0,
                intensity: Stereo::symmetric(IntensityChange::DoNotChange),
            };
        }

        self.pending.a = self.pending.a.then(intensity.a);
        self.pending.b = self.pending.b.then(intensity.b);

        let intensity = Stereo {
            a: std::mem::take(&mut self.pending.a).into_change(),
            b: std::mem::take(&mut self.pending.b).into_change(),
        };
        if intensity == Stereo::symmetric(IntensityChange::DoNotChange) {
            return PreparedIntensity {
                sequence: 0,
                intensity,
            };
        }

        let sequence = self.next_sequence;
        self.next_sequence = if sequence == 15 { 1 } else { sequence + 1 };
        let prepared = PreparedIntensity {
            sequence,
            intensity,
        };
        self.outstanding = Some(prepared);
        prepared
    }

    fn acknowledge(&mut self, sequence: u8) -> bool {
        if sequence != 0
            && self
                .outstanding
                .is_some_and(|sent| sent.sequence == sequence)
        {
            self.outstanding = None;
            true
        } else {
            false
        }
    }

    fn rollback(&mut self, prepared: PreparedIntensity) {
        if prepared.sequence == 0
            || !self
                .outstanding
                .is_some_and(|sent| sent.sequence == prepared.sequence)
        {
            return;
        }

        self.outstanding = None;
        self.pending.a = self.pending.a.prepend(prepared.intensity.a);
        self.pending.b = self.pending.b.prepend(prepared.intensity.b);
    }

    fn is_waiting(&self) -> bool {
        self.outstanding.is_some()
    }

    fn queue_after_outstanding(
        pending: PendingIntensityChange,
        outstanding: IntensityChange,
        change: IntensityChange,
    ) -> PendingIntensityChange {
        if change == IntensityChange::DoNotChange {
            return pending;
        }

        let pending = match (pending, outstanding) {
            (PendingIntensityChange::None, IntensityChange::AbsoluteChange(value)) => {
                PendingIntensityChange::Absolute(i16::from(value.min(200)))
            }
            _ => pending,
        };
        pending.then(change)
    }
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
            | IntensityChange::AbsoluteChange(v) => (*v).min(200),
        }
    }
}

#[derive(Clone, Copy, Debug, binrw::BinWrite)]
#[bw(big)]
struct B0Frame {
    #[bw(map = |prepared| (
        (prepared.sequence << 4)
            | (prepared.intensity.a.mode() << 2)
            | prepared.intensity.b.mode(),
        prepared.intensity.a.value(),
        prepared.intensity.b.value(),
    ))]
    intensity: PreparedIntensity,
    pulses: [[u8; 4]; 4],
}

impl From<Pulses> for B0Frame {
    fn from(pulses: Pulses) -> Self {
        Self {
            intensity: PreparedIntensity {
                sequence: 0,
                intensity: pulses.intensity,
            },
            pulses: Pulses::convert_pulses(&pulses.pulses),
        }
    }
}

#[derive(Clone, Copy, Debug, binrw::BinWrite)]
#[bw(big)]
enum Command {
    #[bw(magic = 0xB0u8)]
    SendPulses(B0Frame),
    #[bw(magic = 0xBFu8)]
    UpdateSettings(DeviceSettings),
}
impl Command {
    fn to_bytes(mut self) -> impl Deref<Target = [u8]> {
        use binrw::BinWrite;

        if let Command::UpdateSettings(settings) = &mut self {
            *settings = settings.normalized();
        }

        let mut buf = ArrayVec::<u8, 20>::new_const();
        self.write_be(&mut binrw::io::NoSeek::new(&mut buf))
            .expect("writing must not fail");
        buf
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Notification {
    IntensityChange { serial: u8, intensity: Stereo<u8> },
    DeviceSettingsChange(DeviceSettings),
}

fn parse_notification(value: &[u8]) -> Option<Notification> {
    match value {
        [0xb1, serial, a, b] => Some(Notification::IntensityChange {
            serial: *serial,
            intensity: Stereo { a: *a, b: *b },
        }),
        [
            0xbe,
            limit_a,
            limit_b,
            frequency_a,
            frequency_b,
            intensity_a,
            intensity_b,
        ] => Some(Notification::DeviceSettingsChange(DeviceSettings {
            limit: Stereo {
                a: *limit_a,
                b: *limit_b,
            },
            frequency_balance: Stereo {
                a: *frequency_a,
                b: *frequency_b,
            },
            intensity_balance: Stereo {
                a: *intensity_a,
                b: *intensity_b,
            },
        })),
        _ => None,
    }
}

fn apply_protocol_notification(
    state: &mut State,
    sequencer: &mut IntensitySequencer,
    notification: Notification,
) {
    match notification {
        Notification::IntensityChange { serial, intensity } => {
            sequencer.acknowledge(serial);
            state.intensity = intensity;
            state.last_intensity_sequence = serial;
            state.intensity_update_pending = sequencer.is_waiting();
        }
        Notification::DeviceSettingsChange(_) => {
            debug!("ignoring deprecated Coyote BE settings notification");
        }
    }
}

fn parse_battery_payload(value: &[u8]) -> Option<u8> {
    match value {
        [level] => Some(*level),
        _ => None,
    }
}

fn validate_characteristic_uuids(uuids: impl IntoIterator<Item = Uuid>) -> Result<()> {
    let uuids: Vec<_> = uuids.into_iter().collect();
    for required in [
        BATTERY_CHARACTERISTIC_UUID,
        NOTIFY_CHARACTERISTIC_UUID,
        WRITE_CHARACTERISTIC_UUID,
    ] {
        if !uuids.contains(&required) {
            return Err(Error::MissingCharacteristic(required));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use hex_literal::hex;

    #[test]
    fn test_b0_command() {
        assert_eq!(
            &*Command::SendPulses(
                Pulses {
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
                }
                .into()
            )
            .to_bytes(),
            hex!("b00f0a000a0a0a0a000000002121212100000000")
        );
        assert_eq!(
            &*Command::SendPulses(
                Pulses {
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
                }
                .into()
            )
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
