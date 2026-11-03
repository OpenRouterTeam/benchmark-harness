use std::path::Path;

use catalog::Catalog;

/// Every service compiled into the world must match its catalog entry exactly, so the
/// MCP servers' env (which comes from the catalog) points at the right port and hostnames.
#[test]
fn implemented_services_match_the_catalog() {
    let catalog =
        Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/services.toml"))
            .unwrap();
    for service in world::services() {
        let entry = catalog
            .services
            .iter()
            .find(|s| s.name == service.name())
            .unwrap_or_else(|| panic!("{} is not in the catalog", service.name()));
        assert_eq!(service.port(), entry.world_port, "{} port", service.name());
        assert_eq!(
            service.hosts(),
            entry.hosts.as_slice(),
            "{} hosts",
            service.name()
        );
    }
}
