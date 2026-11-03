use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{Map, Value, json};

use crate::{
    clock::LogicalClock,
    dataset::{Dataset, ScenarioMeta},
    error::Result,
    ids::IdGen,
    journal::Journal,
    store::Store,
};

/// Mutable state of one mock service.
#[derive(Debug, Clone)]
pub struct ServiceState {
    pub store: Store,
    pub ids: IdGen,
}

/// Everything a running world knows. Exactly one scenario per world.
#[derive(Debug, Clone)]
pub struct WorldState {
    pub meta: ScenarioMeta,
    pub clock: LogicalClock,
    pub journal: Journal,
    /// The agent's final answer, posted by the verifier before grading.
    pub answer: Option<String>,
    services: BTreeMap<String, ServiceState>,
    source: Dataset,
}

pub type SharedWorld = Arc<Mutex<WorldState>>;

pub fn shared(state: WorldState) -> SharedWorld {
    Arc::new(Mutex::new(state))
}

impl WorldState {
    pub fn from_dataset(dataset: Dataset) -> Result<Self> {
        let meta = dataset.meta.clone();
        let clock = LogicalClock::new(&meta.now, meta.clock_step_secs)?;
        let services = dataset
            .services
            .iter()
            .map(|(name, store)| {
                let state = ServiceState {
                    store: store.clone(),
                    ids: IdGen::new(service_seed(&meta.seed, name)),
                };
                (name.clone(), state)
            })
            .collect();
        Ok(Self {
            meta,
            clock,
            journal: Journal::default(),
            answer: None,
            services,
            source: dataset,
        })
    }

    pub fn service(&self, name: &str) -> Option<&ServiceState> {
        self.services.get(name)
    }

    /// The named service's state, created empty if the dataset had none.
    pub fn service_mut(&mut self, name: &str) -> &mut ServiceState {
        let seed = service_seed(&self.meta.seed, name);
        self.services
            .entry(name.to_owned())
            .or_insert_with(|| ServiceState {
                store: Store::default(),
                ids: IdGen::new(seed),
            })
    }

    /// Back to the scenario's starting state: dataset, clock, IDs, and an empty journal.
    pub fn reset(&mut self) {
        *self =
            Self::from_dataset(self.source.clone()).expect("dataset was valid when first loaded");
    }

    pub fn snapshot(&self) -> Value {
        let services = self
            .services
            .iter()
            .map(|(name, state)| (name.clone(), state.store.snapshot()))
            .collect::<Map<_, _>>();
        json!({
            "scenario": self.meta.id,
            "now": self.clock.now_rfc3339(),
            "services": services,
        })
    }
}

fn service_seed(seed: &str, service: &str) -> String {
    format!("{seed}/{service}")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;

    fn example() -> WorldState {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/example/github/basic");
        WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap()
    }

    #[test]
    fn snapshot_reports_scenario_time_and_services() {
        let world = example();
        let snap = world.snapshot();
        assert_eq!(snap["scenario"], json!("example/github/basic"));
        assert_eq!(snap["now"], json!("2026-03-14T09:00:00Z"));
        assert_eq!(
            snap["services"]["github"]["repos"]["octo-org/app"]["id"],
            json!(1296269)
        );
    }

    #[test]
    fn service_mut_creates_missing_services_with_seeded_ids() {
        let mut a = example();
        let mut b = example();
        assert!(a.service("stripe").is_none());
        let id_a = a
            .service_mut("stripe")
            .ids
            .next_prefixed("customer", "cus_", 14);
        let id_b = b
            .service_mut("stripe")
            .ids
            .next_prefixed("customer", "cus_", 14);
        assert_eq!(id_a, id_b);
        let github_id = a
            .service_mut("github")
            .ids
            .next_prefixed("customer", "cus_", 14);
        assert_ne!(github_id, id_a, "services get independent ID streams");
    }

    #[test]
    fn reset_restores_dataset_clock_and_journal() {
        let mut world = example();
        world.clock.tick();
        world
            .service_mut("github")
            .store
            .collection_mut("issues")
            .remove("octo-org/app#1");
        world.reset();
        assert_eq!(world.clock.now_rfc3339(), "2026-03-14T09:00:00Z");
        assert!(world.journal.entries().is_empty());
        assert!(
            world
                .service("github")
                .unwrap()
                .store
                .collection("issues")
                .unwrap()
                .contains("octo-org/app#1")
        );
    }
}
