//! Factory wiring: CoreKind → core provider, AddonSourceId → addon source.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cubecli_core::error::{CoreError, Result};
use cubecli_core::model::{AddonSourceId, CoreKind};
use cubecli_core::traits::addon_source::AddonSource;
use cubecli_core::traits::server_core::ServerCoreProvider;

use crate::curseforge::{CurseForgeSource, KeyProvider};
use crate::fabric::FabricProvider;
use crate::forge::ForgeProvider;
use crate::modrinth::ModrinthSource;
use crate::mojang::{MojangMeta, MojangProvider};
use crate::neoforge::NeoForgeProvider;
use crate::paper::PaperProvider;
use crate::purpur::PurpurProvider;

pub struct CoreRegistry {
    cores: HashMap<CoreKind, Arc<dyn ServerCoreProvider>>,
    mojang: Arc<MojangMeta>,
    pub client: reqwest::Client,
}

impl CoreRegistry {
    pub fn new(client: reqwest::Client) -> Self {
        let mojang = Arc::new(MojangMeta::new(client.clone()));
        let cores: HashMap<CoreKind, Arc<dyn ServerCoreProvider>> = HashMap::from([
            (
                CoreKind::Vanilla,
                Arc::new(MojangProvider::new(mojang.clone())) as Arc<dyn ServerCoreProvider>,
            ),
            (
                CoreKind::Paper,
                Arc::new(PaperProvider::new(client.clone())) as Arc<dyn ServerCoreProvider>,
            ),
            (
                CoreKind::Purpur,
                Arc::new(PurpurProvider::new(client.clone(), mojang.clone()))
                    as Arc<dyn ServerCoreProvider>,
            ),
            (
                CoreKind::Fabric,
                Arc::new(FabricProvider::new(client.clone(), mojang.clone()))
                    as Arc<dyn ServerCoreProvider>,
            ),
            (
                CoreKind::NeoForge,
                Arc::new(NeoForgeProvider::new(client.clone(), mojang.clone()))
                    as Arc<dyn ServerCoreProvider>,
            ),
            (
                CoreKind::Forge,
                Arc::new(ForgeProvider::new(client.clone(), mojang.clone()))
                    as Arc<dyn ServerCoreProvider>,
            ),
        ]);
        Self {
            cores,
            mojang,
            client,
        }
    }

    pub fn get(&self, kind: CoreKind) -> Result<Arc<dyn ServerCoreProvider>> {
        self.cores
            .get(&kind)
            .cloned()
            .ok_or_else(|| CoreError::not_found("cubecli", format!("no provider for core {kind}")))
    }

    pub fn mojang(&self) -> &Arc<MojangMeta> {
        &self.mojang
    }
}

/// Addon sources. The CurseForge key is resolved lazily on every call so
/// `cubecli setup curseforge` takes effect without restarting anything.
pub struct AddonRegistry {
    addons: HashMap<AddonSourceId, Arc<dyn AddonSource>>,
}

impl AddonRegistry {
    pub fn new(client: reqwest::Client, curseforge_key: KeyProvider, state_dir: PathBuf) -> Self {
        let addons: HashMap<AddonSourceId, Arc<dyn AddonSource>> = HashMap::from([
            (
                AddonSourceId::Modrinth,
                Arc::new(ModrinthSource::new(client.clone())) as Arc<dyn AddonSource>,
            ),
            (
                AddonSourceId::CurseForge,
                Arc::new(CurseForgeSource::new(
                    client,
                    curseforge_key,
                    state_dir.join("cf-categories.json"),
                )) as Arc<dyn AddonSource>,
            ),
        ]);
        Self { addons }
    }

    pub fn get(&self, id: AddonSourceId) -> Result<Arc<dyn AddonSource>> {
        self.addons
            .get(&id)
            .cloned()
            .ok_or_else(|| CoreError::not_found("cubecli", format!("no addon source {id}")))
    }
}
