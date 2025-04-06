pub(crate) trait PeripheralExt: btleplug::api::Peripheral {
    async fn local_name_matches(&self, name: &str) -> btleplug::Result<bool> {
        let properties = self.properties().await?;

        Ok(properties
            .and_then(|p| p.local_name)
            .is_some_and(|local_name| local_name == name))
    }
}

impl<T: btleplug::api::Peripheral> PeripheralExt for T {}
