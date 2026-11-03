//! Composition root: everything a command needs, wired with concrete types.

use std::sync::Arc;

use cubecli_core::{FsJavaLocator, GlobalConfig, GlobalLayout};
use cubecli_providers::adoptium::AdoptiumClient;
use cubecli_providers::download::HttpFetcher;
use cubecli_providers::http;
use cubecli_providers::{AddonRegistry, CoreRegistry};

use crate::exit::UserError;

pub struct AppContext {
    pub global: GlobalLayout,
    pub config: GlobalConfig,
    pub cores: Arc<CoreRegistry>,
    pub addons: Arc<AddonRegistry>,
    pub fetcher: Arc<HttpFetcher>,
    pub java: FsJavaLocator,
    pub java_installer: Arc<AdoptiumClient>,
}

impl AppContext {
    pub fn load() -> Result<Self, UserError> {
        let global = GlobalLayout::discover()?;
        global
            .ensure_dirs()
            .map_err(|e| UserError::from(cubecli_core::CoreError::io(global.root(), e)))?;
        let config = GlobalConfig::load(global.config_file())?;
        let client = http::client();
        let cores = Arc::new(CoreRegistry::new(client.clone()));
        // The CurseForge key re-reads config+env on every call, so
        // `cubecli setup curseforge` takes effect immediately.
        let config_path = global.config_file();
        let cf_key: cubecli_providers::curseforge::KeyProvider = Arc::new(move || {
            GlobalConfig::load(&config_path)
                .ok()
                .and_then(|c| c.curseforge_key())
        });
        let addons = Arc::new(AddonRegistry::new(
            client.clone(),
            cf_key,
            global.root().to_path_buf(),
        ));
        let fetcher = Arc::new(HttpFetcher::new(client.clone()));
        let java = FsJavaLocator::new(global.jdks_dir());
        let java_installer = Arc::new(AdoptiumClient::new(client, global.jdks_dir()));
        Ok(Self {
            global,
            config,
            cores,
            addons,
            fetcher,
            java,
            java_installer,
        })
    }
}
