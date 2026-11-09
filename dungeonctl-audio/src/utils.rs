pub(crate) fn cancel() -> (CancellationSender, CancellationReceiver) {
    let (tx, rx) = async_channel::bounded(1);
    (CancellationSender(rx), CancellationReceiver(tx))
}

#[derive(Clone)]
pub(crate) struct CancellationReceiver(async_channel::Sender<()>);

impl CancellationReceiver {
    pub fn is_canceled(&self) -> bool {
        self.0.is_closed()
    }
}

#[derive(Debug)]
pub(crate) struct CancellationSender(#[allow(dead_code)] async_channel::Receiver<()>);
