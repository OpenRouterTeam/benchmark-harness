use std::{collections::BTreeSet, path::Path};

use catalog::{Catalog, Mode, Runtime};

fn real() -> Catalog {
    Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/services.toml"))
        .unwrap()
}

#[test]
fn has_thirty_services_in_six_groups_of_five() {
    let c = real();
    assert_eq!(c.services.len(), 30);
    let groups: BTreeSet<&str> = c.services.iter().map(|s| s.group.as_str()).collect();
    assert_eq!(groups.len(), 6);
    for g in &groups {
        assert_eq!(
            c.services.iter().filter(|s| s.group == *g).count(),
            5,
            "group {g}"
        );
    }
}

#[test]
fn validates_cleanly() {
    real().validate().unwrap();
}

#[test]
fn replica_and_real_endpoints_are_well_formed() {
    let c = real();
    for s in &c.services {
        for e in &s.endpoints {
            match s.mode {
                Mode::Replica => {
                    assert_eq!(e.runtime, Runtime::Replica, "{}", e.id);
                    assert!(e.snapshot.is_some(), "{}", e.id);
                }
                _ => {
                    assert_ne!(e.runtime, Runtime::Replica, "{}", e.id);
                    assert!(
                        e.command.is_some() && e.package.is_some() && e.version.is_some(),
                        "{}",
                        e.id
                    );
                }
            }
        }
        if s.mode == Mode::RealTls {
            assert!(!s.hosts.is_empty(), "{}", s.name);
        }
    }
}

#[test]
fn launch_specs_substitute_world_placeholders() {
    let c = real();
    let gitlab = c.launch("gitlab").unwrap();
    assert_eq!(gitlab.env["GITLAB_API_URL"], "http://world:8107/api/v4");
    let sentry = c.launch("sentry").unwrap();
    assert!(sentry.args.contains(&"--host=world:8113".to_owned()));
    let k8s = c.launch("kubernetes").unwrap();
    assert_eq!(
        k8s.env["KUBECONFIG"],
        "/etc/tool-bench/kubernetes/kubeconfig"
    );
    assert!(c.launch("stripe").is_err(), "replicas have no launch spec");
    assert_eq!(
        gitlab.env["PYTHONHASHSEED"], "0",
        "every server gets a fixed hash seed"
    );
}

#[test]
fn world_aliases_cover_every_tls_host() {
    let c = real();
    let aliases = c.world_aliases();
    assert!(aliases.contains(&"api.search.brave.com".to_owned()));
    assert!(aliases.contains(&"github.octo-corp.com".to_owned()));
    let unique: BTreeSet<_> = aliases.iter().collect();
    assert_eq!(unique.len(), aliases.len());
}

#[test]
fn validation_catches_duplicate_ports_and_unknown_placeholders() {
    let toml = r#"
[[service]]
name = "a"
group = "g"
mode = "real"
world_port = 8101
hosts = []
[[service.endpoint]]
id = "a"
mcp_port = 7101
runtime = "npm"
package = "a"
version = "1"
command = "a"
env = { X = "{nope}" }

[[service]]
name = "b"
group = "g"
mode = "replica"
world_port = 8101
hosts = []
[[service.endpoint]]
id = "b"
mcp_port = 7101
runtime = "replica"
snapshot = "b.json"
"#;
    let err = Catalog::parse(toml)
        .unwrap()
        .validate()
        .unwrap_err()
        .to_string();
    assert!(err.contains("world_port 8101 used twice"), "{err}");
    assert!(err.contains("mcp_port 7101 used twice"), "{err}");
    assert!(err.contains("unknown placeholder {nope}"), "{err}");
}
