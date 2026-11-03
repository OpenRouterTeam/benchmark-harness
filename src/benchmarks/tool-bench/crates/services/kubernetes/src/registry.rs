//! The API resources this cluster serves: built-in groups plus every CustomResourceDefinition
//! in the dataset. Discovery and REST routing both read from here.

use mock_core::store::Store;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct ResType {
    pub group: String,
    pub version: String,
    pub plural: String,
    pub singular: String,
    pub kind: String,
    pub namespaced: bool,
    pub verbs: Vec<String>,
    pub short_names: Vec<String>,
    pub categories: Vec<String>,
    pub subresources: Vec<(String, String, Vec<String>)>,
    /// Extra table columns for custom resources: (name, type, jsonPath, priority).
    pub printer_columns: Vec<(String, String, String, i64)>,
    pub builtin: bool,
}

impl ResType {
    pub fn group_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    /// Store collection: `pods`, `deployments.apps`, `pipelineruns.tekton.dev`.
    pub fn collection(&self) -> String {
        if self.group.is_empty() {
            self.plural.clone()
        } else {
            format!("{}.{}", self.plural, self.group)
        }
    }

    /// Name used in error messages, e.g. `deployments.apps`.
    pub fn qualified(&self) -> String {
        self.collection()
    }

    pub fn has_verb(&self, verb: &str) -> bool {
        self.verbs.iter().any(|v| v == verb)
    }

    pub fn has_sub(&self, sub: &str) -> bool {
        self.subresources.iter().any(|(s, _, _)| s == sub)
    }
}

const RW: &[&str] = &[
    "create",
    "delete",
    "deletecollection",
    "get",
    "list",
    "patch",
    "update",
    "watch",
];
const RO: &[&str] = &["get", "list"];
const CREATE: &[&str] = &["create"];

struct Def {
    group: &'static str,
    version: &'static str,
    plural: &'static str,
    kind: &'static str,
    namespaced: bool,
    verbs: &'static [&'static str],
    short: &'static [&'static str],
    all: bool,
    subs: &'static [&'static str],
}

#[allow(clippy::too_many_arguments)]
const fn d(
    group: &'static str,
    version: &'static str,
    plural: &'static str,
    kind: &'static str,
    namespaced: bool,
    short: &'static [&'static str],
    all: bool,
    subs: &'static [&'static str],
) -> Def {
    Def {
        group,
        version,
        plural,
        kind,
        namespaced,
        verbs: RW,
        short,
        all,
        subs,
    }
}

const BUILTIN: &[Def] = &[
    d("", "v1", "bindings", "Binding", true, &[], false, &[]),
    Def {
        verbs: RO,
        ..d(
            "",
            "v1",
            "componentstatuses",
            "ComponentStatus",
            false,
            &["cs"],
            false,
            &[],
        )
    },
    d(
        "",
        "v1",
        "configmaps",
        "ConfigMap",
        true,
        &["cm"],
        false,
        &[],
    ),
    d("", "v1", "endpoints", "Endpoints", true, &["ep"], true, &[]),
    d("", "v1", "events", "Event", true, &["ev"], false, &[]),
    d(
        "",
        "v1",
        "limitranges",
        "LimitRange",
        true,
        &["limits"],
        false,
        &[],
    ),
    d(
        "",
        "v1",
        "namespaces",
        "Namespace",
        false,
        &["ns"],
        false,
        &["finalize", "status"],
    ),
    d(
        "",
        "v1",
        "nodes",
        "Node",
        false,
        &["no"],
        false,
        &["proxy", "status"],
    ),
    d(
        "",
        "v1",
        "persistentvolumeclaims",
        "PersistentVolumeClaim",
        true,
        &["pvc"],
        true,
        &["status"],
    ),
    d(
        "",
        "v1",
        "persistentvolumes",
        "PersistentVolume",
        false,
        &["pv"],
        false,
        &["status"],
    ),
    d(
        "",
        "v1",
        "pods",
        "Pod",
        true,
        &["po"],
        true,
        &[
            "attach",
            "binding",
            "ephemeralcontainers",
            "eviction",
            "exec",
            "log",
            "portforward",
            "proxy",
            "resize",
            "status",
        ],
    ),
    d(
        "",
        "v1",
        "podtemplates",
        "PodTemplate",
        true,
        &[],
        false,
        &[],
    ),
    d(
        "",
        "v1",
        "replicationcontrollers",
        "ReplicationController",
        true,
        &["rc"],
        true,
        &["scale", "status"],
    ),
    d(
        "",
        "v1",
        "resourcequotas",
        "ResourceQuota",
        true,
        &["quota"],
        false,
        &["status"],
    ),
    d("", "v1", "secrets", "Secret", true, &[], false, &[]),
    d(
        "",
        "v1",
        "serviceaccounts",
        "ServiceAccount",
        true,
        &["sa"],
        false,
        &["token"],
    ),
    d(
        "",
        "v1",
        "services",
        "Service",
        true,
        &["svc"],
        true,
        &["proxy", "status"],
    ),
    d(
        "apps",
        "v1",
        "controllerrevisions",
        "ControllerRevision",
        true,
        &[],
        false,
        &[],
    ),
    d(
        "apps",
        "v1",
        "daemonsets",
        "DaemonSet",
        true,
        &["ds"],
        true,
        &["status"],
    ),
    d(
        "apps",
        "v1",
        "deployments",
        "Deployment",
        true,
        &["deploy"],
        true,
        &["scale", "status"],
    ),
    d(
        "apps",
        "v1",
        "replicasets",
        "ReplicaSet",
        true,
        &["rs"],
        true,
        &["scale", "status"],
    ),
    d(
        "apps",
        "v1",
        "statefulsets",
        "StatefulSet",
        true,
        &["sts"],
        true,
        &["scale", "status"],
    ),
    d(
        "autoscaling",
        "v2",
        "horizontalpodautoscalers",
        "HorizontalPodAutoscaler",
        true,
        &["hpa"],
        true,
        &["status"],
    ),
    d(
        "autoscaling",
        "v1",
        "horizontalpodautoscalers",
        "HorizontalPodAutoscaler",
        true,
        &["hpa"],
        true,
        &["status"],
    ),
    d(
        "batch",
        "v1",
        "cronjobs",
        "CronJob",
        true,
        &["cj"],
        true,
        &["status"],
    ),
    d("batch", "v1", "jobs", "Job", true, &[], true, &["status"]),
    d(
        "networking.k8s.io",
        "v1",
        "ingressclasses",
        "IngressClass",
        false,
        &[],
        false,
        &[],
    ),
    d(
        "networking.k8s.io",
        "v1",
        "ingresses",
        "Ingress",
        true,
        &["ing"],
        false,
        &["status"],
    ),
    d(
        "networking.k8s.io",
        "v1",
        "networkpolicies",
        "NetworkPolicy",
        true,
        &["netpol"],
        false,
        &[],
    ),
    d(
        "policy",
        "v1",
        "poddisruptionbudgets",
        "PodDisruptionBudget",
        true,
        &["pdb"],
        false,
        &["status"],
    ),
    d(
        "rbac.authorization.k8s.io",
        "v1",
        "clusterrolebindings",
        "ClusterRoleBinding",
        false,
        &[],
        false,
        &[],
    ),
    d(
        "rbac.authorization.k8s.io",
        "v1",
        "clusterroles",
        "ClusterRole",
        false,
        &[],
        false,
        &[],
    ),
    d(
        "rbac.authorization.k8s.io",
        "v1",
        "rolebindings",
        "RoleBinding",
        true,
        &[],
        false,
        &[],
    ),
    d(
        "rbac.authorization.k8s.io",
        "v1",
        "roles",
        "Role",
        true,
        &[],
        false,
        &[],
    ),
    d(
        "storage.k8s.io",
        "v1",
        "csidrivers",
        "CSIDriver",
        false,
        &[],
        false,
        &[],
    ),
    d(
        "storage.k8s.io",
        "v1",
        "csinodes",
        "CSINode",
        false,
        &[],
        false,
        &[],
    ),
    d(
        "storage.k8s.io",
        "v1",
        "storageclasses",
        "StorageClass",
        false,
        &["sc"],
        false,
        &[],
    ),
    d(
        "storage.k8s.io",
        "v1",
        "volumeattachments",
        "VolumeAttachment",
        false,
        &[],
        false,
        &["status"],
    ),
    d(
        "apiextensions.k8s.io",
        "v1",
        "customresourcedefinitions",
        "CustomResourceDefinition",
        false,
        &["crd", "crds"],
        false,
        &["status"],
    ),
    d(
        "apiregistration.k8s.io",
        "v1",
        "apiservices",
        "APIService",
        false,
        &[],
        false,
        &["status"],
    ),
    Def {
        verbs: CREATE,
        ..d(
            "authentication.k8s.io",
            "v1",
            "selfsubjectreviews",
            "SelfSubjectReview",
            false,
            &[],
            false,
            &[],
        )
    },
    Def {
        verbs: CREATE,
        ..d(
            "authentication.k8s.io",
            "v1",
            "tokenreviews",
            "TokenReview",
            false,
            &[],
            false,
            &[],
        )
    },
    Def {
        verbs: CREATE,
        ..d(
            "authorization.k8s.io",
            "v1",
            "localsubjectaccessreviews",
            "LocalSubjectAccessReview",
            true,
            &[],
            false,
            &[],
        )
    },
    Def {
        verbs: CREATE,
        ..d(
            "authorization.k8s.io",
            "v1",
            "selfsubjectaccessreviews",
            "SelfSubjectAccessReview",
            false,
            &[],
            false,
            &[],
        )
    },
    Def {
        verbs: CREATE,
        ..d(
            "authorization.k8s.io",
            "v1",
            "selfsubjectrulesreviews",
            "SelfSubjectRulesReview",
            false,
            &[],
            false,
            &[],
        )
    },
    Def {
        verbs: CREATE,
        ..d(
            "authorization.k8s.io",
            "v1",
            "subjectaccessreviews",
            "SubjectAccessReview",
            false,
            &[],
            false,
            &[],
        )
    },
    d(
        "certificates.k8s.io",
        "v1",
        "certificatesigningrequests",
        "CertificateSigningRequest",
        false,
        &["csr"],
        false,
        &["approval", "status"],
    ),
    d(
        "coordination.k8s.io",
        "v1",
        "leases",
        "Lease",
        true,
        &[],
        false,
        &[],
    ),
    d(
        "discovery.k8s.io",
        "v1",
        "endpointslices",
        "EndpointSlice",
        true,
        &[],
        false,
        &[],
    ),
    d(
        "events.k8s.io",
        "v1",
        "events",
        "Event",
        true,
        &["ev"],
        false,
        &[],
    ),
    d(
        "node.k8s.io",
        "v1",
        "runtimeclasses",
        "RuntimeClass",
        false,
        &[],
        false,
        &[],
    ),
    d(
        "scheduling.k8s.io",
        "v1",
        "priorityclasses",
        "PriorityClass",
        false,
        &["pc"],
        false,
        &[],
    ),
    d(
        "admissionregistration.k8s.io",
        "v1",
        "mutatingwebhookconfigurations",
        "MutatingWebhookConfiguration",
        false,
        &[],
        false,
        &[],
    ),
    d(
        "admissionregistration.k8s.io",
        "v1",
        "validatingwebhookconfigurations",
        "ValidatingWebhookConfiguration",
        false,
        &[],
        false,
        &[],
    ),
    Def {
        verbs: RO,
        ..d(
            "metrics.k8s.io",
            "v1beta1",
            "nodes",
            "NodeMetrics",
            false,
            &[],
            false,
            &[],
        )
    },
    Def {
        verbs: RO,
        ..d(
            "metrics.k8s.io",
            "v1beta1",
            "pods",
            "PodMetrics",
            true,
            &[],
            false,
            &[],
        )
    },
];

fn singular_of(kind: &str) -> String {
    kind.to_ascii_lowercase()
}

fn sub_kind(kind: &str, sub: &str) -> (String, Vec<String>) {
    match sub {
        "scale" => (
            "Scale".into(),
            vec!["get".into(), "patch".into(), "update".into()],
        ),
        "status" => (
            kind.into(),
            vec!["get".into(), "patch".into(), "update".into()],
        ),
        "log" => ("Pod".into(), vec!["get".into()]),
        "eviction" => ("Eviction".into(), vec!["create".into()]),
        "binding" => ("Binding".into(), vec!["create".into()]),
        "token" => ("TokenRequest".into(), vec!["create".into()]),
        "exec" | "attach" | "portforward" => {
            ("PodExecOptions".into(), vec!["create".into(), "get".into()])
        }
        "proxy" => (
            format!("{kind}ProxyOptions"),
            ["create", "delete", "get", "patch", "update"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
        "finalize" | "approval" => (kind.into(), vec!["update".into()]),
        _ => (
            kind.into(),
            vec!["get".into(), "patch".into(), "update".into()],
        ),
    }
}

/// Every resource type the cluster serves, built-ins first, then custom resources in
/// dataset order.
pub fn all_types(store: Option<&Store>) -> Vec<ResType> {
    let mut out: Vec<ResType> = BUILTIN
        .iter()
        .map(|def| ResType {
            group: def.group.into(),
            version: def.version.into(),
            plural: def.plural.into(),
            singular: singular_of(def.kind),
            kind: def.kind.into(),
            namespaced: def.namespaced,
            verbs: def.verbs.iter().map(|s| s.to_string()).collect(),
            short_names: def.short.iter().map(|s| s.to_string()).collect(),
            categories: if def.all { vec!["all".into()] } else { vec![] },
            subresources: def
                .subs
                .iter()
                .map(|s| {
                    let (k, v) = sub_kind(def.kind, s);
                    (s.to_string(), k, v)
                })
                .collect(),
            printer_columns: vec![],
            builtin: true,
        })
        .collect();
    if let Some(crds) =
        store.and_then(|s| s.collection("customresourcedefinitions.apiextensions.k8s.io"))
    {
        for crd in crds.values() {
            out.extend(types_from_crd(crd));
        }
    }
    out
}

fn types_from_crd(crd: &Value) -> Vec<ResType> {
    let spec = &crd["spec"];
    let group = spec["group"].as_str().unwrap_or_default().to_owned();
    let names = &spec["names"];
    let plural = names["plural"].as_str().unwrap_or_default().to_owned();
    let kind = names["kind"].as_str().unwrap_or_default().to_owned();
    if group.is_empty() || plural.is_empty() || kind.is_empty() {
        return vec![];
    }
    let strs = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let namespaced = spec["scope"].as_str() != Some("Cluster");
    let mut versions: Vec<&Value> = spec["versions"]
        .as_array()
        .map(|v| v.iter().filter(|v| v["served"] != false).collect())
        .unwrap_or_default();
    // Storage version first, so it is the preferred version.
    versions.sort_by_key(|v| v["storage"] != true);
    versions
        .into_iter()
        .filter_map(|v| {
            let version = v["name"].as_str()?.to_owned();
            let mut subresources = Vec::new();
            if v["subresources"].get("status").is_some() {
                subresources.push((
                    "status".into(),
                    kind.clone(),
                    vec!["get".into(), "patch".into(), "update".into()],
                ));
            }
            if v["subresources"].get("scale").is_some() {
                subresources.push((
                    "scale".into(),
                    "Scale".into(),
                    vec!["get".into(), "patch".into(), "update".into()],
                ));
            }
            let printer_columns = v["additionalPrinterColumns"]
                .as_array()
                .map(|cols| {
                    cols.iter()
                        .map(|c| {
                            (
                                c["name"].as_str().unwrap_or_default().to_owned(),
                                c["type"].as_str().unwrap_or("string").to_owned(),
                                c["jsonPath"].as_str().unwrap_or_default().to_owned(),
                                c["priority"].as_i64().unwrap_or(0),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(ResType {
                group: group.clone(),
                version,
                plural: plural.clone(),
                singular: names["singular"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| kind.to_ascii_lowercase()),
                kind: kind.clone(),
                namespaced,
                verbs: RW.iter().map(|s| s.to_string()).collect(),
                short_names: strs(&names["shortNames"]),
                categories: strs(&names["categories"]),
                subresources,
                printer_columns,
                builtin: false,
            })
        })
        .collect()
}

/// `(group, [versions in preference order])` in discovery order.
pub fn groups(types: &[ResType]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for t in types.iter().filter(|t| !t.group.is_empty()) {
        match out.iter_mut().find(|(g, _)| *g == t.group) {
            Some((_, versions)) => {
                if !versions.contains(&t.version) {
                    versions.push(t.version.clone());
                }
            }
            None => out.push((t.group.clone(), vec![t.version.clone()])),
        }
    }
    out
}

pub fn find<'a>(
    types: &'a [ResType],
    group: &str,
    version: &str,
    plural: &str,
) -> Option<&'a ResType> {
    types
        .iter()
        .find(|t| t.group == group && t.version == version && t.plural == plural)
}

/// The type stored under `collection` with the preferred version.
pub fn by_collection<'a>(types: &'a [ResType], collection: &str) -> Option<&'a ResType> {
    types.iter().find(|t| t.collection() == collection)
}

pub fn resource_list(types: &[ResType], group: &str, version: &str) -> Option<Value> {
    let matching: Vec<&ResType> = types
        .iter()
        .filter(|t| t.group == group && t.version == version)
        .collect();
    if matching.is_empty() {
        return None;
    }
    let mut resources = Vec::new();
    for t in matching {
        let mut r = json!({
            "name": t.plural,
            "singularName": t.singular,
            "namespaced": t.namespaced,
            "kind": t.kind,
            "verbs": t.verbs,
        });
        if !t.short_names.is_empty() {
            r["shortNames"] = json!(t.short_names);
        }
        if !t.categories.is_empty() {
            r["categories"] = json!(t.categories);
        }
        if t.builtin {
            r["storageVersionHash"] = json!(storage_hash(t));
        }
        resources.push(r);
        for (sub, kind, verbs) in &t.subresources {
            let mut s = json!({
                "name": format!("{}/{sub}", t.plural),
                "singularName": "",
                "namespaced": t.namespaced,
                "kind": kind,
                "verbs": verbs,
            });
            if sub == "scale" {
                s["group"] = json!("autoscaling");
                s["version"] = json!("v1");
            } else if sub == "eviction" {
                s["group"] = json!("policy");
                s["version"] = json!("v1");
            } else if sub == "token" {
                s["group"] = json!("authentication.k8s.io");
                s["version"] = json!("v1");
            }
            resources.push(s);
        }
    }
    let group_version = if group.is_empty() {
        version.to_owned()
    } else {
        format!("{group}/{version}")
    };
    Some(json!({
        "kind": "APIResourceList",
        "apiVersion": "v1",
        "groupVersion": group_version,
        "resources": resources,
    }))
}

fn storage_hash(t: &ResType) -> String {
    let h = crate::util::fnv(&format!("{}/{}", t.group_version(), t.plural));
    crate::util::b64_encode(&h.to_be_bytes())[..11].to_owned()
}
