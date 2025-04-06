use std::{
    pin::{Pin, pin},
    sync::{Arc, Mutex, RwLock},
};

use futures::{Stream, StreamExt};
use futures_signals::signal::Signal;

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

impl<T: Clone> DeviceState<T> {
    pub(crate) fn new(stream: impl Stream<Item = T> + Send + Unpin + 'static, default: T) -> Self {
        Self {
            stream: Arc::new(Mutex::from(stream)),
            inner: Arc::new(RwLock::new(default)),
        }
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

        match pin!(&mut *stream).poll_next_unpin(cx) {
            std::task::Poll::Pending | std::task::Poll::Ready(None) => std::task::Poll::Pending,
            std::task::Poll::Ready(Some(v)) => {
                let mut inner = self.inner.write().unwrap();
                if v == *inner {
                    std::task::Poll::Pending
                } else {
                    *inner = v.clone();
                    std::task::Poll::Ready(Some(v))
                }
            }
        }
    }
}
