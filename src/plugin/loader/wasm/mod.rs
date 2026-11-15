pub mod wasm_host;

use std::path::Path;
use std::sync::Arc;

use crate::plugin::api::PluginMetadata;
use wasm_host::state::ProxyPluginContext;
pub use wasm_host::{PluginRuntime, WasmPlugin};

pub struct WasmPluginLoader {
    runtime: PluginRuntime,
}

impl WasmPluginLoader {
    pub fn new(plugins_dir: &Path) -> Result<Self, wasm_host::PluginInitError> {
        let runtime = PluginRuntime::new(plugins_dir)?;
        Ok(Self { runtime })
    }

    #[must_use]
    pub fn can_load(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wasm"))
    }

    pub async fn load(
        &self,
        path: &Path,
        proxy: Option<ProxyPluginContext>,
    ) -> Result<(Arc<WasmPlugin>, PluginMetadata), wasm_host::PluginInitError> {
        self.runtime.init_plugin(path, proxy).await
    }
}
