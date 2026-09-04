use std::{
    pin::{Pin, pin},
    sync::{Arc, Mutex, RwLock},
};

use futures::{Stream, StreamExt};
use futures_signals::signal::{Mutable, Signal, SignalExt};

/// A reactive [`Signal`] whose value can also be read out directly.
///
/// [`Signal`]: https://docs.rs/futures-signals/latest/futures_signals/tutorial/index.html#signal-1
#[allow(private_bounds)]
pub trait StateSignal<T>: Signal<Item = T> + super::Sealed {
    /// Get the current value.
    fn get(&self) -> T;
}

/// A reactive [`StateSignal`] that can be created from a stream of value updates.
#[derive(Clone)]
pub(crate) struct DeviceState<T> {
    stream: Arc<Mutex<dyn Stream<Item = T> + Send + Unpin + 'static>>,
    inner: Arc<RwLock<T>>,
}

#[derive(Clone, Debug)]
pub(crate) struct StatePublisher<T> {
    signal: Mutable<T>,
    inner: Arc<RwLock<T>>,
    update_lock: Arc<Mutex<()>>,
}

impl<T: Clone> DeviceState<T> {
    pub(crate) fn channel(default: T) -> (Self, StatePublisher<T>)
    where
        T: PartialEq + Send + Sync + 'static,
    {
        let signal = Mutable::new(default.clone());
        let state = Self {
            stream: Arc::new(Mutex::new(signal.signal_cloned().to_stream())),
            inner: Arc::new(RwLock::new(default)),
        };
        let publisher = StatePublisher {
            signal,
            inner: Arc::clone(&state.inner),
            update_lock: Arc::new(Mutex::new(())),
        };

        (state, publisher)
    }
}

impl<T: Clone + PartialEq> StatePublisher<T> {
    pub(crate) fn update(&self, update: impl FnOnce(&mut T)) {
        self.update_with_hook(update, || {});
    }

    fn update_with_hook(&self, update: impl FnOnce(&mut T), before_publication: impl FnOnce()) {
        let _update_guard = self.update_lock.lock().unwrap();
        let value = {
            let mut value = self.inner.write().unwrap();
            update(&mut value);
            value.clone()
        };
        before_publication();
        self.signal.set_neq(value);
    }
}

impl<T: Clone + std::fmt::Debug> std::fmt::Debug for DeviceState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DeviceState").field(&self.inner).finish()
    }
}

impl<T> super::Sealed for DeviceState<T> {}

impl<T: Clone + PartialEq + Unpin> StateSignal<T> for DeviceState<T> {
    fn get(&self) -> T {
        self.inner.read().unwrap().clone()
    }
}

impl<T: Clone + PartialEq + Unpin> Signal for DeviceState<T> {
    type Item = T;

    fn poll_change(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut stream = self.stream.lock().unwrap();

        pin!(&mut *stream).poll_next_unpin(cx)
    }
}
