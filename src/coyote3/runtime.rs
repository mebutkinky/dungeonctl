//! A small pulse runtime for a connected Coyote 3.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::task::JoinHandle;
use tracing::{debug, error};

use crate::{
    Result, StateSignal, Stereo,
    coyote3::{Coyote3, EncodedPulse, EncodedPulses, IntensityChange, Pulse, State},
};

/// The current channel values passed to a [`Shape`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeContext {
    /// Which 25 ms chunk inside the current 100 ms frame this is.
    ///
    /// Valid values are `0`, `1`, `2`, and `3`.
    pub chunk_index: u8,

    /// The previous pulse frequency for this channel.
    pub frequency: u8,

    /// The current absolute device intensity for this channel.
    pub channel_intensity: u8,

    /// The previous pulse intensity for this channel.
    pub pulse_intensity: u8,
}

/// The current channel values passed to an [`IntensityShape`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntensityContext {
    /// The current absolute device intensity for this channel.
    pub channel_intensity: u8,
}

/// Generates the next 25 ms pulse for one channel.
pub trait Shape: Send + 'static {
    /// Return the next 25 ms pulse for the supplied current channel values.
    fn next_pulse(&mut self, context: ShapeContext) -> Pulse;
}

impl<F> Shape for F
where
    F: FnMut(ShapeContext) -> Pulse + Send + 'static,
{
    fn next_pulse(&mut self, context: ShapeContext) -> Pulse {
        self(context)
    }
}

/// The current channel values passed to an [`EncodedShape`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncodedShapeContext {
    /// Which 25 ms chunk inside the current 100 ms frame this is.
    pub chunk_index: u8,
    /// The previous compressed V3 frequency value for this channel.
    pub frequency: u8,
    /// The current absolute device intensity for this channel.
    pub channel_intensity: u8,
    /// The previous pulse intensity for this channel.
    pub pulse_intensity: u8,
}

/// Generates a protocol-native 25 ms pulse for one channel.
pub trait EncodedShape: Send + 'static {
    /// Return the next compressed V3 pulse for the supplied channel values.
    fn next_pulse(&mut self, context: EncodedShapeContext) -> EncodedPulse;
}

impl<F> EncodedShape for F
where
    F: FnMut(EncodedShapeContext) -> EncodedPulse + Send + 'static,
{
    fn next_pulse(&mut self, context: EncodedShapeContext) -> EncodedPulse {
        self(context)
    }
}

/// Generates the next 100 ms intensity change for one channel.
///
/// This is called once per frame, before the four pulse chunks are rendered.
pub trait IntensityShape: Send + 'static {
    /// Return the next intensity change for this 100 ms frame.
    fn next_intensity(&mut self, context: IntensityContext) -> IntensityChange;
}

impl<F> IntensityShape for F
where
    F: FnMut(IntensityContext) -> IntensityChange + Send + 'static,
{
    fn next_intensity(&mut self, context: IntensityContext) -> IntensityChange {
        self(context)
    }
}

/// Runs a Coyote 3 pulse loop until stopped.
///
/// Dropping this value aborts its background transmission task. Call [`Runtime::stop`] for a
/// graceful shutdown that also sends a disabled waveform frame.
pub struct Runtime {
    coyote: Arc<Coyote3>,
    core: Arc<Mutex<RuntimeCore>>,
    task: Mutex<TaskGuard>,
    state_callback: StateCallbackSlot,
}

type StateCallback = Box<dyn FnMut(State) + Send>;

#[derive(Clone, Default)]
struct StateCallbackSlot(Arc<Mutex<StateCallbackState>>);

#[derive(Default)]
struct StateCallbackState {
    generation: u64,
    callback: Option<StateCallback>,
}

impl StateCallbackSlot {
    fn set(&self, callback: impl FnMut(State) + Send + 'static) {
        let mut state = self.0.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.callback = Some(Box::new(callback));
    }

    fn clear(&self) {
        let mut state = self.0.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.callback = None;
    }

    fn invoke(&self, value: State) {
        let (generation, callback) = {
            let mut state = self.0.lock().unwrap();
            (state.generation, state.callback.take())
        };
        let Some(mut callback) = callback else {
            return;
        };

        callback(value);

        let mut state = self.0.lock().unwrap();
        if state.generation == generation && state.callback.is_none() {
            state.callback = Some(callback);
        }
    }

    #[cfg(test)]
    fn has_callback(&self) -> bool {
        self.0.lock().unwrap().callback.is_some()
    }
}

impl Runtime {
    /// Create a runtime for a connected Coyote 3.
    pub fn new(coyote: Coyote3) -> Self {
        Self {
            coyote: Arc::new(coyote),
            core: Arc::default(),
            task: Mutex::new(TaskGuard::default()),
            state_callback: StateCallbackSlot::default(),
        }
    }

    /// Start sending frames every 100 ms.
    ///
    /// Each frame contains one intensity change and four 25 ms pulse chunks.
    ///
    /// Calling this while already started leaves the existing runtime task running.
    pub fn start(&self) {
        let mut task = self.task.lock().unwrap();
        task.clear_if_finished();
        if !task.is_empty() {
            return;
        }

        let coyote = Arc::clone(&self.coyote);
        let core = Arc::clone(&self.core);
        let state_callback = self.state_callback.clone();

        task.replace(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));

            loop {
                interval.tick().await;

                let state = coyote.state().get();

                state_callback.invoke(state);

                let channel_intensity = state.intensity;

                let command = core.lock().unwrap().render_frame(channel_intensity);

                if let Err(error) = coyote.send_encoded_pulses(command).await {
                    error!(?error, "stopping coyote runtime after send failure");
                    break;
                }
            }
        }));
    }

    /// Stop sending pulses and send one zeroed pulse frame.
    pub async fn stop(&self) -> Result<()> {
        self.task.lock().unwrap().abort();

        debug!("stopping coyote runtime");

        self.coyote
            .send_encoded_pulses(EncodedPulses {
                intensity: Stereo::symmetric(IntensityChange::DoNotChange),
                pulses: [Stereo::symmetric(EncodedPulse {
                    frequency: 0,
                    intensity: 0,
                }); 4],
            })
            .await
    }

    /// Set a callback that is called once per 100 ms frame with the current device state.
    ///
    /// The callback runs inside the runtime task before rendering/sending the pulse frame.
    pub fn set_state_callback(&self, callback: impl FnMut(State) + Send + 'static) {
        self.state_callback.set(callback);
    }

    /// Remove the current state callback.
    pub fn clear_state_callback(&self) {
        self.state_callback.clear();
    }

    /// Replace the pulse shape used for channel A.
    pub fn set_shape_a(&self, shape: impl Shape) {
        self.core.lock().unwrap().set_shape_a(shape);
    }

    /// Replace the pulse shape used for channel B.
    pub fn set_shape_b(&self, shape: impl Shape) {
        self.core.lock().unwrap().set_shape_b(shape);
    }

    /// Replace the protocol-native pulse shape used for channel A.
    pub fn set_encoded_shape_a(&self, shape: impl EncodedShape) {
        self.core.lock().unwrap().set_encoded_shape_a(shape);
    }

    /// Replace the protocol-native pulse shape used for channel B.
    pub fn set_encoded_shape_b(&self, shape: impl EncodedShape) {
        self.core.lock().unwrap().set_encoded_shape_b(shape);
    }

    /// Replace the intensity shape used for channel A.
    pub fn set_intensity_shape_a(&self, shape: impl IntensityShape) {
        self.core.lock().unwrap().set_intensity_shape_a(shape);
    }

    /// Replace the intensity shape used for channel B.
    pub fn set_intensity_shape_b(&self, shape: impl IntensityShape) {
        self.core.lock().unwrap().set_intensity_shape_b(shape);
    }
}

#[derive(Default)]
struct TaskGuard(Option<JoinHandle<()>>);

impl TaskGuard {
    fn replace(&mut self, task: JoinHandle<()>) {
        self.abort();
        self.0 = Some(task);
    }

    fn abort(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }

    fn clear_if_finished(&mut self) -> bool {
        if self.0.as_ref().is_some_and(JoinHandle::is_finished) {
            self.0.take();
            true
        } else {
            false
        }
    }

    fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

impl From<JoinHandle<()>> for TaskGuard {
    fn from(task: JoinHandle<()>) -> Self {
        Self(Some(task))
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.abort();
    }
}

pub(crate) struct RuntimeCore {
    a: ChannelRuntime,
    b: ChannelRuntime,
}

impl Default for RuntimeCore {
    fn default() -> Self {
        Self {
            a: ChannelRuntime::new(ZeroShape, DoNotChangeIntensityShape),
            b: ChannelRuntime::new(ZeroShape, DoNotChangeIntensityShape),
        }
    }
}

impl RuntimeCore {
    pub(crate) fn set_shape_a(&mut self, shape: impl Shape) {
        self.a.set_shape(shape);
    }

    pub(crate) fn set_shape_b(&mut self, shape: impl Shape) {
        self.b.set_shape(shape);
    }

    pub(crate) fn set_encoded_shape_a(&mut self, shape: impl EncodedShape) {
        self.a.set_encoded_shape(shape);
    }

    pub(crate) fn set_encoded_shape_b(&mut self, shape: impl EncodedShape) {
        self.b.set_encoded_shape(shape);
    }

    pub(crate) fn set_intensity_shape_a(&mut self, shape: impl IntensityShape) {
        self.a.set_intensity_shape(shape);
    }

    pub(crate) fn set_intensity_shape_b(&mut self, shape: impl IntensityShape) {
        self.b.set_intensity_shape(shape);
    }

    pub(crate) fn render_frame(&mut self, channel_intensity: Stereo<u8>) -> EncodedPulses {
        let intensity = Stereo {
            a: self.a.next_intensity(channel_intensity.a),
            b: self.b.next_intensity(channel_intensity.b),
        };

        let pulses = std::array::from_fn(|index| {
            let chunk_index = index as u8;

            Stereo {
                a: self.a.next_pulse(chunk_index, channel_intensity.a),
                b: self.b.next_pulse(chunk_index, channel_intensity.b),
            }
        });

        EncodedPulses { intensity, pulses }
    }
}

struct ChannelRuntime {
    shape: ChannelShape,
    intensity_shape: Box<dyn IntensityShape>,
}

enum ChannelShape {
    Hertz {
        shape: Box<dyn Shape>,
        current: Pulse,
    },
    Encoded {
        shape: Box<dyn EncodedShape>,
        current: EncodedPulse,
    },
}

impl ChannelRuntime {
    fn new(shape: impl Shape, intensity_shape: impl IntensityShape) -> Self {
        Self {
            shape: ChannelShape::Hertz {
                shape: Box::new(shape),
                current: Pulse {
                    frequency: 0,
                    intensity: 0,
                },
            },
            intensity_shape: Box::new(intensity_shape),
        }
    }

    fn set_shape(&mut self, shape: impl Shape) {
        self.shape = ChannelShape::Hertz {
            shape: Box::new(shape),
            current: Pulse {
                frequency: 0,
                intensity: 0,
            },
        };
    }

    fn set_encoded_shape(&mut self, shape: impl EncodedShape) {
        self.shape = ChannelShape::Encoded {
            shape: Box::new(shape),
            current: EncodedPulse {
                frequency: 0,
                intensity: 0,
            },
        };
    }

    fn set_intensity_shape(&mut self, shape: impl IntensityShape) {
        self.intensity_shape = Box::new(shape);
    }

    fn next_intensity(&mut self, channel_intensity: u8) -> IntensityChange {
        self.intensity_shape
            .next_intensity(IntensityContext { channel_intensity })
    }

    fn next_pulse(&mut self, chunk_index: u8, channel_intensity: u8) -> EncodedPulse {
        match &mut self.shape {
            ChannelShape::Hertz { shape, current } => {
                *current = shape.next_pulse(ShapeContext {
                    chunk_index,
                    frequency: current.frequency,
                    channel_intensity,
                    pulse_intensity: current.intensity,
                });
                EncodedPulse {
                    frequency: current.encoded_frequency(),
                    intensity: current.clamped_intensity(),
                }
            }
            ChannelShape::Encoded { shape, current } => {
                *current = shape.next_pulse(EncodedShapeContext {
                    chunk_index,
                    frequency: current.frequency,
                    channel_intensity,
                    pulse_intensity: current.intensity,
                });
                let (frequency, intensity) = current.normalized();
                EncodedPulse {
                    frequency,
                    intensity,
                }
            }
        }
    }
}

/// Disables the pulse
pub struct ZeroShape;

impl Shape for ZeroShape {
    fn next_pulse(&mut self, _context: ShapeContext) -> Pulse {
        Pulse {
            frequency: 0,
            intensity: 0,
        }
    }
}

/// Leaves the device-level channel intensity unchanged on every frame.
pub struct DoNotChangeIntensityShape;

impl IntensityShape for DoNotChangeIntensityShape {
    fn next_intensity(&mut self, _context: IntensityContext) -> IntensityChange {
        IntensityChange::DoNotChange
    }
}
