use btleplug::{
    api::{Central as _, CentralEvent, Manager as _, Peripheral as _},
    platform::{Adapter, Manager, Peripheral},
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture};
use tracing::debug;

use crate::Result;

/// Builder type to connect to a BLE device.
///
/// This type implements [`IntoFuture`], so you just need to `.await` it to start the connection.
pub struct BleDeviceConnectionBuilder<
    T,
    S: Default,
    F: AsyncFnOnce(Adapter, Peripheral, S) -> Result<T>,
> {
    device_name: &'static str,
    adapter: Option<Adapter>,
    peripheral: Option<Peripheral>,
    settings: S,
    init: F,
}

impl<T, S: Default, F: AsyncFnOnce(Adapter, Peripheral, S) -> Result<T>>
    BleDeviceConnectionBuilder<T, S, F>
{
    pub(crate) fn new(device_name: &'static str, init: F) -> Self {
        Self {
            device_name,
            adapter: None,
            peripheral: None,
            settings: S::default(),
            init,
        }
    }
}

impl<T, S: Default, F: AsyncFnOnce(Adapter, Peripheral, S) -> Result<T>>
    BleDeviceConnectionBuilder<T, S, F>
{
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
    pub fn settings(mut self, settings: S) -> Self {
        self.settings = settings;
        self
    }
    /// Initiate the connection.
    pub async fn connect(self) -> Result<T> {
        let adapter = match self.adapter {
            Some(adapter) => adapter,
            None => {
                let manager = Manager::new().await.unwrap();
                debug!(?manager, "using default manager");
                manager.adapters().await?.swap_remove(0)
            }
        };
        let peripheral = match self.peripheral {
            Some(peripheral) => peripheral,
            None => {
                debug!("starting BLE scan");
                adapter.start_scan(Default::default()).await?;

                let peripheral = 'peripheral: {
                    let mut events = adapter.events().await?;

                    while let Some(event) = events.next().await {
                        if let CentralEvent::DeviceDiscovered(id) = event {
                            let peripheral = adapter.peripheral(&id).await?;
                            debug!("device discovered: {}", peripheral.address());

                            if peripheral.local_name_matches(self.device_name).await? {
                                break 'peripheral peripheral;
                            }
                        }
                    }

                    unreachable!()
                };

                debug!("stopping BLE scan");
                adapter.stop_scan().await?;

                peripheral
            }
        };

        debug!("connecting to {}", peripheral.address());
        peripheral.connect().await?;
        debug!("discovering services");
        peripheral.discover_services().await?;

        (self.init)(adapter, peripheral, self.settings).await
    }
}

impl<T, S, F> IntoFuture for BleDeviceConnectionBuilder<T, S, F>
where
    T: 'static,
    S: Default + 'static,
    F: AsyncFnOnce(Adapter, Peripheral, S) -> Result<T> + 'static,
{
    type IntoFuture = LocalBoxFuture<'static, Self::Output>;
    type Output = Result<T>;

    fn into_future(self) -> Self::IntoFuture {
        self.connect().boxed_local()
    }
}

trait PeripheralExt: btleplug::api::Peripheral {
    async fn local_name_matches(&self, name: &str) -> btleplug::Result<bool> {
        let properties = self.properties().await?;

        Ok(properties
            .and_then(|p| p.local_name)
            .is_some_and(|local_name| local_name == name))
    }
}

impl<T: btleplug::api::Peripheral> PeripheralExt for T {}
