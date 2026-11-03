//! `application/json;as=Table` rendering with the kube-apiserver's column sets.

use std::borrow::Cow;

use serde_json::{Value, json};

use crate::{
    registry::ResType,
    util::{human_duration, parse_rfc3339},
};

struct Col {
    name: Cow<'static, str>,
    ty: Cow<'static, str>,
    format: &'static str,
    priority: i64,
    description: &'static str,
}

const fn col(
    name: &'static str,
    ty: &'static str,
    priority: i64,
    description: &'static str,
) -> Col {
    Col {
        name: Cow::Borrowed(name),
        ty: Cow::Borrowed(ty),
        format: "",
        priority,
        description,
    }
}

const NAME_DESC: &str = "Name must be unique within a namespace. Is required when creating resources, although some resources may allow a client to request the generation of an appropriate name automatically. Name is primarily intended for creation idempotence and configuration definition. Cannot be updated. More info: https://kubernetes.io/docs/concepts/overview/working-with-objects/names#names";
const AGE_DESC: &str = "CreationTimestamp is a timestamp representing the server time when this object was created. It is not guaranteed to be set in happens-before order across separate operations. Clients may not set this value. It is represented in RFC3339 form and is in UTC.\n\nPopulated by the system. Read-only. Null for lists. More info: https://git.k8s.io/community/contributors/devel/sig-architecture/api-conventions.md#metadata";

fn name_col() -> Col {
    Col {
        format: "name",
        ..col("Name", "string", 0, NAME_DESC)
    }
}

fn age_col() -> Col {
    col("Age", "string", 0, AGE_DESC)
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn age(obj: &Value, now: i64) -> String {
    obj["metadata"]["creationTimestamp"]
        .as_str()
        .and_then(parse_rfc3339)
        .map(|t| human_duration(now - t))
        .unwrap_or_else(|| "<unknown>".into())
}

fn none_if_empty(s: String) -> String {
    if s.is_empty() { "<none>".into() } else { s }
}

fn selector_string(sel: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(m) = sel.get("matchLabels").and_then(Value::as_object) {
        for (k, v) in m {
            parts.push(format!("{k}={}", s(v)));
        }
    } else if let Some(m) = sel.as_object() {
        for (k, v) in m {
            if k != "matchExpressions" && v.is_string() {
                parts.push(format!("{k}={}", s(v)));
            }
        }
    }
    none_if_empty(parts.join(","))
}

fn containers_images(spec: &Value) -> (String, String) {
    let containers = spec["containers"].as_array().cloned().unwrap_or_default();
    let names: Vec<String> = containers.iter().map(|c| s(&c["name"])).collect();
    let images: Vec<String> = containers.iter().map(|c| s(&c["image"])).collect();
    (names.join(","), images.join(","))
}

/// The kubectl `STATUS` column for a pod.
pub fn pod_status(pod: &Value) -> String {
    let status = &pod["status"];
    let mut reason = status["reason"]
        .as_str()
        .or_else(|| status["phase"].as_str())
        .unwrap_or("Unknown")
        .to_owned();
    let mut initializing = false;
    if let Some(inits) = status["initContainerStatuses"].as_array() {
        let total = inits.len();
        for (i, c) in inits.iter().enumerate() {
            let term = &c["state"]["terminated"];
            if term.is_object() && int(&term["exitCode"]) == 0 {
                continue;
            }
            initializing = true;
            if term.is_object() {
                reason = term["reason"]
                    .as_str()
                    .map(|r| format!("Init:{r}"))
                    .unwrap_or_else(|| format!("Init:ExitCode:{}", int(&term["exitCode"])));
            } else if let Some(r) = c["state"]["waiting"]["reason"].as_str()
                && r != "PodInitializing"
            {
                reason = format!("Init:{r}");
            } else {
                reason = format!("Init:{i}/{total}");
            }
            break;
        }
    }
    if !initializing {
        let mut has_running = false;
        if let Some(cs) = status["containerStatuses"].as_array() {
            for c in cs.iter().rev() {
                let state = &c["state"];
                if let Some(r) = state["waiting"]["reason"].as_str() {
                    reason = r.to_owned();
                } else if state["terminated"].is_object() {
                    let t = &state["terminated"];
                    reason = t["reason"].as_str().map(str::to_owned).unwrap_or_else(|| {
                        if int(&t["signal"]) != 0 {
                            format!("Signal:{}", int(&t["signal"]))
                        } else {
                            format!("ExitCode:{}", int(&t["exitCode"]))
                        }
                    });
                } else if state["running"].is_object() && c["ready"] == true {
                    has_running = true;
                }
            }
            if reason == "Completed" && has_running {
                reason = "Running".into();
            }
        }
    }
    if pod["metadata"]["deletionTimestamp"].is_string() {
        reason = if status["reason"] == "NodeLost" {
            "Unknown".into()
        } else {
            "Terminating".into()
        };
    }
    reason
}

fn pod_restarts(pod: &Value, now: i64) -> String {
    let mut restarts = 0;
    let mut last: Option<i64> = None;
    for c in pod["status"]["containerStatuses"]
        .as_array()
        .into_iter()
        .flatten()
    {
        restarts += int(&c["restartCount"]);
        if let Some(t) = c["lastState"]["terminated"]["finishedAt"]
            .as_str()
            .and_then(parse_rfc3339)
        {
            last = Some(last.map_or(t, |l: i64| l.max(t)));
        }
    }
    match last {
        Some(t) if restarts > 0 => format!("{restarts} ({} ago)", human_duration(now - t)),
        _ => restarts.to_string(),
    }
}

fn row_cells(rt: &ResType, obj: &Value, now: i64) -> (Vec<Col>, Vec<Value>) {
    let name = || json!(s(&obj["metadata"]["name"]));
    let spec = &obj["spec"];
    let status = &obj["status"];
    match (rt.group.as_str(), rt.plural.as_str()) {
        ("", "pods") => {
            let cs = status["containerStatuses"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let total = spec["containers"].as_array().map_or(cs.len(), Vec::len);
            let ready = cs.iter().filter(|c| c["ready"] == true).count();
            (
                vec![
                    name_col(),
                    col(
                        "Ready",
                        "string",
                        0,
                        "The aggregate readiness state of this pod for accepting traffic.",
                    ),
                    col(
                        "Status",
                        "string",
                        0,
                        "The aggregate status of the containers in this pod.",
                    ),
                    col(
                        "Restarts",
                        "string",
                        0,
                        "The number of times the containers in this pod have been restarted and when the last container in this pod has restarted.",
                    ),
                    age_col(),
                    col("IP", "string", 1, "The IP address assigned to the pod"),
                    col(
                        "Node",
                        "string",
                        1,
                        "The name of the node assigned to the pod",
                    ),
                    col(
                        "Nominated Node",
                        "string",
                        1,
                        "The name of the node nominated by the scheduler",
                    ),
                    col(
                        "Readiness Gates",
                        "string",
                        1,
                        "The readiness gates of the pod",
                    ),
                ],
                vec![
                    name(),
                    json!(format!("{ready}/{total}")),
                    json!(pod_status(obj)),
                    json!(pod_restarts(obj, now)),
                    json!(age(obj, now)),
                    json!(none_if_empty(s(&status["podIP"]))),
                    json!(none_if_empty(s(&spec["nodeName"]))),
                    json!("<none>"),
                    json!("<none>"),
                ],
            )
        }
        ("apps", "deployments") => {
            let (c, i) = containers_images(&spec["template"]["spec"]);
            (
                vec![
                    name_col(),
                    col("Ready", "string", 0, "Number of the pod with ready state"),
                    col(
                        "Up-to-date",
                        "string",
                        0,
                        "Total number of non-terminated pods targeted by this deployment that have the desired template spec.",
                    ),
                    col(
                        "Available",
                        "string",
                        0,
                        "Total number of available pods (ready for at least minReadySeconds) targeted by this deployment.",
                    ),
                    age_col(),
                    col(
                        "Containers",
                        "string",
                        1,
                        "Names of each container in the template.",
                    ),
                    col(
                        "Images",
                        "string",
                        1,
                        "Images referenced by each container in the template.",
                    ),
                    col(
                        "Selector",
                        "string",
                        1,
                        "Label selector for pods. Existing ReplicaSets whose pods are selected by this will be the ones affected by this deployment.",
                    ),
                ],
                vec![
                    name(),
                    json!(format!(
                        "{}/{}",
                        int(&status["readyReplicas"]),
                        spec["replicas"].as_i64().unwrap_or(1)
                    )),
                    json!(int(&status["updatedReplicas"])),
                    json!(int(&status["availableReplicas"])),
                    json!(age(obj, now)),
                    json!(c),
                    json!(i),
                    json!(selector_string(&spec["selector"])),
                ],
            )
        }
        ("apps", "replicasets") | ("", "replicationcontrollers") => {
            let (c, i) = containers_images(&spec["template"]["spec"]);
            (
                vec![
                    name_col(),
                    col(
                        "Desired",
                        "integer",
                        0,
                        "Replicas is the number of desired replicas.",
                    ),
                    col(
                        "Current",
                        "integer",
                        0,
                        "Replicas is the most recently observed number of replicas.",
                    ),
                    col(
                        "Ready",
                        "integer",
                        0,
                        "The number of ready replicas for this replica set.",
                    ),
                    age_col(),
                    col(
                        "Containers",
                        "string",
                        1,
                        "Names of each container in the template.",
                    ),
                    col(
                        "Images",
                        "string",
                        1,
                        "Images referenced by each container in the template.",
                    ),
                    col(
                        "Selector",
                        "string",
                        1,
                        "Label query over pods that should match the replica count.",
                    ),
                ],
                vec![
                    name(),
                    json!(spec["replicas"].as_i64().unwrap_or(1)),
                    json!(int(&status["replicas"])),
                    json!(int(&status["readyReplicas"])),
                    json!(age(obj, now)),
                    json!(c),
                    json!(i),
                    json!(selector_string(&spec["selector"])),
                ],
            )
        }
        ("apps", "statefulsets") => {
            let (c, i) = containers_images(&spec["template"]["spec"]);
            (
                vec![
                    name_col(),
                    col("Ready", "string", 0, "Number of the pod with ready state"),
                    age_col(),
                    col(
                        "Containers",
                        "string",
                        1,
                        "Names of each container in the template.",
                    ),
                    col(
                        "Images",
                        "string",
                        1,
                        "Images referenced by each container in the template.",
                    ),
                ],
                vec![
                    name(),
                    json!(format!(
                        "{}/{}",
                        int(&status["readyReplicas"]),
                        spec["replicas"].as_i64().unwrap_or(1)
                    )),
                    json!(age(obj, now)),
                    json!(c),
                    json!(i),
                ],
            )
        }
        ("apps", "daemonsets") => {
            let (c, i) = containers_images(&spec["template"]["spec"]);
            let node_selector = spec["template"]["spec"]["nodeSelector"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| format!("{k}={}", s(v)))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            (
                vec![
                    name_col(),
                    col(
                        "Desired",
                        "integer",
                        0,
                        "The total number of nodes that should be running the daemon pod (including nodes correctly running the daemon pod).",
                    ),
                    col(
                        "Current",
                        "integer",
                        0,
                        "The number of nodes that are running at least 1 daemon pod and are supposed to run the daemon pod.",
                    ),
                    col(
                        "Ready",
                        "integer",
                        0,
                        "The number of nodes that should be running the daemon pod and have one or more of the daemon pod running and ready.",
                    ),
                    col(
                        "Up-to-date",
                        "integer",
                        0,
                        "The total number of nodes that are running updated daemon pod",
                    ),
                    col(
                        "Available",
                        "integer",
                        0,
                        "The number of nodes that should be running the daemon pod and have one or more of the daemon pod running and available (ready for at least spec.minReadySeconds)",
                    ),
                    col(
                        "Node Selector",
                        "string",
                        0,
                        "NodeSelector is a selector which must be true for the pod to fit on a node.",
                    ),
                    age_col(),
                    col(
                        "Containers",
                        "string",
                        1,
                        "Names of each container in the template.",
                    ),
                    col(
                        "Images",
                        "string",
                        1,
                        "Images referenced by each container in the template.",
                    ),
                    col(
                        "Selector",
                        "string",
                        1,
                        "A label query over pods that are managed by the daemon set.",
                    ),
                ],
                vec![
                    name(),
                    json!(int(&status["desiredNumberScheduled"])),
                    json!(int(&status["currentNumberScheduled"])),
                    json!(int(&status["numberReady"])),
                    json!(int(&status["updatedNumberScheduled"])),
                    json!(int(&status["numberAvailable"])),
                    json!(none_if_empty(node_selector)),
                    json!(age(obj, now)),
                    json!(c),
                    json!(i),
                    json!(selector_string(&spec["selector"])),
                ],
            )
        }
        ("batch", "jobs") => {
            let (c, i) = containers_images(&spec["template"]["spec"]);
            let completions = spec["completions"].as_i64().unwrap_or(1);
            let job_status = if status["conditions"].as_array().is_some_and(|cs| {
                cs.iter()
                    .any(|c| c["type"] == "Failed" && c["status"] == "True")
            }) {
                "Failed"
            } else if status["completionTime"].is_string() {
                "Complete"
            } else {
                "Running"
            };
            let duration = match (
                status["startTime"].as_str().and_then(parse_rfc3339),
                status["completionTime"].as_str().and_then(parse_rfc3339),
            ) {
                (Some(a), Some(b)) => human_duration(b - a),
                (Some(a), None) => human_duration(now - a),
                _ => String::new(),
            };
            (
                vec![
                    name_col(),
                    col("Status", "string", 0, "Status of the job."),
                    col(
                        "Completions",
                        "string",
                        0,
                        "The desired number of successfully finished pods the job should be run with.",
                    ),
                    col(
                        "Duration",
                        "string",
                        0,
                        "Time required to complete the job.",
                    ),
                    age_col(),
                    col(
                        "Containers",
                        "string",
                        1,
                        "Names of each container in the template.",
                    ),
                    col(
                        "Images",
                        "string",
                        1,
                        "Images referenced by each container in the template.",
                    ),
                    col(
                        "Selector",
                        "string",
                        1,
                        "A label query over pods that should match the pod count.",
                    ),
                ],
                vec![
                    name(),
                    json!(job_status),
                    json!(format!("{}/{completions}", int(&status["succeeded"]))),
                    json!(duration),
                    json!(age(obj, now)),
                    json!(c),
                    json!(i),
                    json!(selector_string(&spec["selector"])),
                ],
            )
        }
        ("batch", "cronjobs") => {
            let (c, i) = containers_images(&spec["jobTemplate"]["spec"]["template"]["spec"]);
            let last = status["lastScheduleTime"]
                .as_str()
                .and_then(parse_rfc3339)
                .map(|t| human_duration(now - t))
                .unwrap_or_else(|| "<none>".into());
            (
                vec![
                    name_col(),
                    col(
                        "Schedule",
                        "string",
                        0,
                        "The schedule in Cron format, see https://en.wikipedia.org/wiki/Cron.",
                    ),
                    col(
                        "Timezone",
                        "string",
                        0,
                        "The time zone name for the given schedule, see https://en.wikipedia.org/wiki/List_of_tz_database_time_zones.",
                    ),
                    col(
                        "Suspend",
                        "boolean",
                        0,
                        "This flag tells the controller to suspend subsequent executions, it does not apply to already started executions.  Defaults to false.",
                    ),
                    col(
                        "Active",
                        "integer",
                        0,
                        "A list of pointers to currently running jobs.",
                    ),
                    col(
                        "Last Schedule",
                        "string",
                        0,
                        "Information when was the last time the job was successfully scheduled.",
                    ),
                    age_col(),
                    col(
                        "Containers",
                        "string",
                        1,
                        "Names of each container in the template.",
                    ),
                    col(
                        "Images",
                        "string",
                        1,
                        "Images referenced by each container in the template.",
                    ),
                    col(
                        "Selector",
                        "string",
                        1,
                        "A label query over pods that should match the pod count.",
                    ),
                ],
                vec![
                    name(),
                    json!(s(&spec["schedule"])),
                    json!(none_if_empty(s(&spec["timeZone"]))),
                    json!(spec["suspend"].as_bool().unwrap_or(false)),
                    json!(status["active"].as_array().map_or(0, Vec::len)),
                    json!(last),
                    json!(age(obj, now)),
                    json!(c),
                    json!(i),
                    json!("<none>"),
                ],
            )
        }
        ("", "services") => {
            let ports = spec["ports"]
                .as_array()
                .map(|ps| {
                    ps.iter()
                        .map(|p| {
                            let proto = p["protocol"].as_str().unwrap_or("TCP");
                            match p["nodePort"].as_i64() {
                                Some(np) => format!("{}:{np}/{proto}", int(&p["port"])),
                                None => format!("{}/{proto}", int(&p["port"])),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let external = status["loadBalancer"]["ingress"]
                .as_array()
                .map(|i| {
                    i.iter()
                        .map(|x| {
                            s(if x["ip"].is_string() {
                                &x["ip"]
                            } else {
                                &x["hostname"]
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if spec["type"] == "LoadBalancer" {
                        "<pending>".into()
                    } else {
                        "<none>".into()
                    }
                });
            (
                vec![
                    name_col(),
                    col(
                        "Type",
                        "string",
                        0,
                        "type determines how the Service is exposed.",
                    ),
                    col(
                        "Cluster-IP",
                        "string",
                        0,
                        "clusterIP is the IP address of the service and is usually assigned randomly.",
                    ),
                    col(
                        "External-IP",
                        "string",
                        0,
                        "externalIPs is a list of IP addresses for which nodes in the cluster will also accept traffic for this service.",
                    ),
                    col(
                        "Port(s)",
                        "string",
                        0,
                        "The list of ports that are exposed by this service.",
                    ),
                    age_col(),
                    col(
                        "Selector",
                        "string",
                        1,
                        "Route service traffic to pods with label keys and values matching this selector.",
                    ),
                ],
                vec![
                    name(),
                    json!(spec["type"].as_str().unwrap_or("ClusterIP")),
                    json!(none_if_empty(s(&spec["clusterIP"]))),
                    json!(external),
                    json!(none_if_empty(ports)),
                    json!(age(obj, now)),
                    json!(selector_string(&spec["selector"])),
                ],
            )
        }
        ("", "configmaps") => {
            let n = obj["data"].as_object().map_or(0, |m| m.len())
                + obj["binaryData"].as_object().map_or(0, |m| m.len());
            (
                vec![
                    name_col(),
                    col("Data", "string", 0, "Data contains the configuration data."),
                    age_col(),
                ],
                vec![name(), json!(n), json!(age(obj, now))],
            )
        }
        ("", "secrets") => {
            let n = obj["data"].as_object().map_or(0, |m| m.len());
            (
                vec![
                    name_col(),
                    col(
                        "Type",
                        "string",
                        0,
                        "Used to facilitate programmatic handling of secret data.",
                    ),
                    col("Data", "string", 0, "Data contains the secret data."),
                    age_col(),
                ],
                vec![
                    name(),
                    json!(obj["type"].as_str().unwrap_or("Opaque")),
                    json!(n),
                    json!(age(obj, now)),
                ],
            )
        }
        ("", "namespaces") => (
            vec![
                name_col(),
                col("Status", "string", 0, "The status of the namespace"),
                age_col(),
            ],
            vec![
                name(),
                json!(status["phase"].as_str().unwrap_or("Active")),
                json!(age(obj, now)),
            ],
        ),
        ("", "nodes") => {
            let ready = status["conditions"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["type"] == "Ready"))
                .map(|c| {
                    if c["status"] == "True" {
                        "Ready"
                    } else {
                        "NotReady"
                    }
                })
                .unwrap_or("Unknown");
            let mut st = ready.to_owned();
            if spec["unschedulable"] == true {
                st.push_str(",SchedulingDisabled");
            }
            let roles = obj["metadata"]["labels"]
                .as_object()
                .map(|m| {
                    m.keys()
                        .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/"))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let addr = |t: &str| {
                status["addresses"]
                    .as_array()
                    .and_then(|a| a.iter().find(|x| x["type"] == t))
                    .map(|x| s(&x["address"]))
                    .unwrap_or_else(|| "<none>".into())
            };
            let info = &status["nodeInfo"];
            (
                vec![
                    name_col(),
                    col("Status", "string", 0, "The status of the node"),
                    col("Roles", "string", 0, "The roles of the node"),
                    age_col(),
                    col(
                        "Version",
                        "string",
                        0,
                        "Kubelet Version reported by the node.",
                    ),
                    col(
                        "Internal-IP",
                        "string",
                        1,
                        "List of addresses reachable to the node.",
                    ),
                    col(
                        "External-IP",
                        "string",
                        1,
                        "List of addresses reachable to the node.",
                    ),
                    col(
                        "OS-Image",
                        "string",
                        1,
                        "OS Image reported by the node from /etc/os-release (e.g. Debian GNU/Linux 7 (wheezy)).",
                    ),
                    col(
                        "Kernel-Version",
                        "string",
                        1,
                        "Kernel Version reported by the node from 'uname -r' (e.g. 3.16.0-0.bpo.4-amd64).",
                    ),
                    col(
                        "Container-Runtime",
                        "string",
                        1,
                        "ContainerRuntime Version reported by the node through runtime remote API (e.g. containerd://1.4.2).",
                    ),
                ],
                vec![
                    name(),
                    json!(st),
                    json!(none_if_empty(roles)),
                    json!(age(obj, now)),
                    json!(s(&info["kubeletVersion"])),
                    json!(addr("InternalIP")),
                    json!(addr("ExternalIP")),
                    json!(s(&info["osImage"])),
                    json!(s(&info["kernelVersion"])),
                    json!(s(&info["containerRuntimeVersion"])),
                ],
            )
        }
        ("", "events") => {
            let last = obj["lastTimestamp"]
                .as_str()
                .or_else(|| obj["eventTime"].as_str())
                .or_else(|| obj["firstTimestamp"].as_str())
                .and_then(parse_rfc3339)
                .map(|t| human_duration(now - t))
                .unwrap_or_else(|| "<unknown>".into());
            let io = &obj["involvedObject"];
            (
                vec![
                    col(
                        "Last Seen",
                        "string",
                        0,
                        "The time at which the most recent occurrence of this event was recorded.",
                    ),
                    col(
                        "Type",
                        "string",
                        0,
                        "Type of this event (Normal, Warning), new types could be added in the future.",
                    ),
                    col(
                        "Reason",
                        "string",
                        0,
                        "This should be a short, machine understandable string that gives the reason for the transition into the object's current status.",
                    ),
                    col(
                        "Object",
                        "string",
                        0,
                        "The object that this event is about.",
                    ),
                    col(
                        "Message",
                        "string",
                        0,
                        "A human-readable description of the status of this operation.",
                    ),
                    col(
                        "Source",
                        "string",
                        1,
                        "The component reporting this event. Should be a short machine understandable string.",
                    ),
                    col(
                        "Subobject",
                        "string",
                        1,
                        "The subobject this event is about.",
                    ),
                    col(
                        "First Seen",
                        "string",
                        1,
                        "The time at which the event was first recorded.",
                    ),
                    col(
                        "Count",
                        "string",
                        1,
                        "The number of times this event has occurred.",
                    ),
                    Col {
                        format: "name",
                        ..col("Name", "string", 1, NAME_DESC)
                    },
                ],
                vec![
                    json!(last),
                    json!(s(&obj["type"])),
                    json!(s(&obj["reason"])),
                    json!(format!(
                        "{}/{}",
                        s(&io["kind"]).to_ascii_lowercase(),
                        s(&io["name"])
                    )),
                    json!(s(&obj["message"])),
                    json!(s(&obj["source"]["component"])),
                    json!(s(&io["fieldPath"])),
                    json!(
                        obj["firstTimestamp"]
                            .as_str()
                            .and_then(parse_rfc3339)
                            .map(|t| human_duration(now - t))
                            .unwrap_or_else(|| "<unknown>".into())
                    ),
                    json!(obj["count"].as_i64().unwrap_or(1).to_string()),
                    name(),
                ],
            )
        }
        ("", "serviceaccounts") => (
            vec![
                name_col(),
                col(
                    "Secrets",
                    "string",
                    0,
                    "Secrets is a list of the secrets in the same namespace that pods running using this ServiceAccount are allowed to use.",
                ),
                age_col(),
            ],
            vec![
                name(),
                json!(obj["secrets"].as_array().map_or(0, Vec::len)),
                json!(age(obj, now)),
            ],
        ),
        ("", "persistentvolumeclaims") => (
            vec![
                name_col(),
                col(
                    "Status",
                    "string",
                    0,
                    "The phase the PersistentVolumeClaim is currently in.",
                ),
                col(
                    "Volume",
                    "string",
                    0,
                    "The binding reference to the PersistentVolume backing this claim.",
                ),
                col(
                    "Capacity",
                    "string",
                    0,
                    "The actual resources of the underlying volume.",
                ),
                col(
                    "Access Modes",
                    "string",
                    0,
                    "AccessModes contains the actual access modes the volume backing the PVC has.",
                ),
                col(
                    "StorageClass",
                    "string",
                    0,
                    "Name of the StorageClass required by the claim.",
                ),
                age_col(),
            ],
            vec![
                name(),
                json!(s(&status["phase"])),
                json!(s(&spec["volumeName"])),
                json!(s(&status["capacity"]["storage"])),
                json!(
                    spec["accessModes"]
                        .as_array()
                        .map(|a| a
                            .iter()
                            .map(|m| match m.as_str() {
                                Some("ReadWriteOnce") => "RWO",
                                Some("ReadOnlyMany") => "ROX",
                                Some("ReadWriteMany") => "RWX",
                                Some("ReadWriteOncePod") => "RWOP",
                                _ => "",
                            })
                            .collect::<Vec<_>>()
                            .join(","))
                        .unwrap_or_default()
                ),
                json!(s(&spec["storageClassName"])),
                json!(age(obj, now)),
            ],
        ),
        ("networking.k8s.io", "ingresses") => {
            let hosts = spec["rules"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .map(|x| s(&x["host"]))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            (
                vec![
                    name_col(),
                    col(
                        "Class",
                        "string",
                        0,
                        "The name of the IngressClass resource that should be used.",
                    ),
                    col(
                        "Hosts",
                        "string",
                        0,
                        "Hosts that incoming requests are matched against before the ingress rule",
                    ),
                    col(
                        "Address",
                        "string",
                        0,
                        "Address is a list containing ingress points for the load-balancer",
                    ),
                    col(
                        "Ports",
                        "string",
                        0,
                        "Ports of TLS configurations that open",
                    ),
                    age_col(),
                ],
                vec![
                    name(),
                    json!(none_if_empty(s(&spec["ingressClassName"]))),
                    json!(if hosts.is_empty() { "*".into() } else { hosts }),
                    json!(
                        status["loadBalancer"]["ingress"]
                            .as_array()
                            .map(|i| i.iter().map(|x| s(&x["ip"])).collect::<Vec<_>>().join(","))
                            .unwrap_or_default()
                    ),
                    json!(if spec["tls"].is_array() {
                        "80, 443"
                    } else {
                        "80"
                    }),
                    json!(age(obj, now)),
                ],
            )
        }
        _ => {
            let mut cols = vec![name_col()];
            let mut cells = vec![name()];
            for (cname, ty, path, priority) in &rt.printer_columns {
                let v = simple_jsonpath(obj, path);
                let cell = if ty == "date" {
                    json!(
                        v.as_str()
                            .and_then(parse_rfc3339)
                            .map(|t| human_duration(now - t))
                            .unwrap_or_default()
                    )
                } else if v.is_null() {
                    json!("")
                } else {
                    v
                };
                cols.push(Col {
                    name: Cow::Owned(cname.clone()),
                    ty: Cow::Owned(ty.clone()),
                    format: "",
                    priority: *priority,
                    description: "",
                });
                cells.push(cell);
            }
            if !rt.printer_columns.iter().any(|(n, _, _, _)| n == "Age") {
                cols.push(age_col());
                cells.push(json!(age(obj, now)));
            }
            (cols, cells)
        }
    }
}

/// `.status.conditions[?(@.type=="Ready")].status`-style paths, enough for CRD columns.
fn simple_jsonpath(obj: &Value, path: &str) -> Value {
    let mut cur = obj.clone();
    let path = path.trim_start_matches('.');
    for part in split_path(path) {
        if let Some((field, filter)) = part.split_once("[?(@.") {
            if !field.is_empty() {
                cur = cur[field].clone();
            }
            let filter = filter.trim_end_matches(")]");
            let (k, v) = filter.split_once("==").unwrap_or((filter, ""));
            let v = v.trim_matches('"').trim_matches('\'');
            cur = cur
                .as_array()
                .and_then(|a| a.iter().find(|x| x[k].as_str() == Some(v)).cloned())
                .unwrap_or(Value::Null);
        } else if let Some((field, idx)) = part.split_once('[') {
            if !field.is_empty() {
                cur = cur[field].clone();
            }
            let i: usize = idx.trim_end_matches(']').parse().unwrap_or(0);
            cur = cur[i].clone();
        } else {
            cur = cur[part].clone();
        }
    }
    cur
}

fn split_path(path: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in path.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '.' if depth == 0 => {
                parts.push(&path[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < path.len() {
        parts.push(&path[start..]);
    }
    parts
}

fn partial_metadata(obj: &Value) -> Value {
    json!({
        "kind": "PartialObjectMetadata",
        "apiVersion": "meta.k8s.io/v1",
        "metadata": obj["metadata"],
    })
}

/// A `meta.k8s.io/v1` Table for `items`.
pub fn render(rt: &ResType, items: &[Value], now: i64, list_meta: Value) -> Value {
    let mut columns: Option<Vec<Col>> = None;
    let mut rows = Vec::new();
    for obj in items {
        let (cols, cells) = row_cells(rt, obj, now);
        columns.get_or_insert(cols);
        rows.push(json!({ "cells": cells, "object": partial_metadata(obj) }));
    }
    let columns = columns.unwrap_or_else(|| row_cells(rt, &json!({"metadata": {}}), now).0);
    let defs: Vec<Value> = columns
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "type": c.ty,
                "format": c.format,
                "description": c.description,
                "priority": c.priority,
            })
        })
        .collect();
    json!({
        "kind": "Table",
        "apiVersion": "meta.k8s.io/v1",
        "metadata": list_meta,
        "columnDefinitions": defs,
        "rows": rows,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pod_status_prefers_waiting_reason() {
        let pod = json!({"status": {"phase": "Running", "containerStatuses": [
            {"name": "app", "ready": false, "restartCount": 7, "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
        ]}});
        assert_eq!(pod_status(&pod), "CrashLoopBackOff");
        let ok = json!({"status": {"phase": "Running", "containerStatuses": [
            {"name": "app", "ready": true, "state": {"running": {}}}
        ]}});
        assert_eq!(pod_status(&ok), "Running");
    }

    #[test]
    fn jsonpath_filters() {
        let obj = json!({"status": {"conditions": [{"type": "A", "status": "x"}, {"type": "Succeeded", "status": "True", "reason": "Done"}]}});
        assert_eq!(
            simple_jsonpath(&obj, ".status.conditions[?(@.type==\"Succeeded\")].reason"),
            json!("Done")
        );
        assert_eq!(
            simple_jsonpath(&obj, ".status.conditions[0].type"),
            json!("A")
        );
    }
}
