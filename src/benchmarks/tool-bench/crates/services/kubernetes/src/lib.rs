//! Mock kube-apiserver for `kubernetes-mcp-server` (client-go, dynamic client, Helm SDK).
//!
//! Serves version and legacy discovery for the built-in groups plus every CRD in the dataset,
//! generic CRUD (get/list/watch/create/update/patch/apply/delete/deletecollection) with label
//! and field selectors and limit/continue paging, `as=Table` output, the scale, status, log,
//! eviction, token and node proxy subresources, authorization reviews, metrics.k8s.io, and
//! Helm releases (stored readably in `helm_releases`, served as `helm.sh/release.v1` Secrets).
//!
//! Dataset layout: one collection per resource, named `<plural>` for the core group and
//! `<plural>.<group>` otherwise (`deployments.apps`, `pods.metrics.k8s.io`), keyed
//! `<namespace>/<name>` or `<name>`. Container logs live in `pod_logs`
//! (`<ns>/<pod>/<container>` -> `{"current": [lines], "previous": [lines]}`), node logs in
//! `node_logs` (`<node>/<query>` -> `{"lines": [...]}`), kubelet stats in `node_stats`.

#![allow(clippy::result_large_err)]

pub mod proto;
pub mod registry;
pub mod table;
pub mod util;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use mock_core::{clock::format_unix, http::json, service::MockService, world::SharedWorld};
use serde_json::{Map, Value, json as json_value};

use crate::{
    registry::{ResType, all_types, find, groups, resource_list},
    util::{
        b64_decode, b64_encode, json_patch, merge_patch, name_suffix, parse_query, stable_uid,
        strategic_merge,
    },
};

pub const NAME: &str = "kubernetes";
pub const PORT: u16 = 8115;
pub const TOKEN: &str = "k8s-toolbench";
pub const USER: &str = "toolbench";

const HELM_COLLECTION: &str = "helm_releases";
const HELM_PREFIX: &str = "sh.helm.release.v1.";
const DEFAULT_CREATED: &str = "2025-01-01T00:00:00Z";

pub struct Kubernetes;

impl MockService for Kubernetes {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new().fallback(handle)
    }
}

// ───────────────────────────── responses ─────────────────────────────

fn status_response(
    code: StatusCode,
    reason: &str,
    message: &str,
    details: Option<Value>,
) -> Response {
    let mut body = json_value!({
        "kind": "Status",
        "apiVersion": "v1",
        "metadata": {},
        "status": "Failure",
        "message": message,
        "reason": reason,
    });
    if let Some(d) = details {
        body["details"] = d;
    } else if reason == "NotFound" {
        body["details"] = json_value!({});
    }
    body["code"] = json_value!(code.as_u16());
    json(code, body)
}

fn not_found_generic() -> Response {
    status_response(
        StatusCode::NOT_FOUND,
        "NotFound",
        "the server could not find the requested resource",
        None,
    )
}

fn details(rt: &ResType, name: &str) -> Value {
    let mut d = json_value!({ "name": name });
    if !rt.group.is_empty() {
        d["group"] = json_value!(rt.group);
    }
    d["kind"] = json_value!(rt.plural);
    d
}

fn not_found(rt: &ResType, name: &str) -> Response {
    status_response(
        StatusCode::NOT_FOUND,
        "NotFound",
        &format!("{} \"{name}\" not found", rt.qualified()),
        Some(details(rt, name)),
    )
}

fn namespace_not_found(ns: &str) -> Response {
    status_response(
        StatusCode::NOT_FOUND,
        "NotFound",
        &format!("namespaces \"{ns}\" not found"),
        Some(json_value!({"name": ns, "kind": "namespaces"})),
    )
}

fn bad_request(message: &str) -> Response {
    status_response(StatusCode::BAD_REQUEST, "BadRequest", message, None)
}

fn invalid(rt: &ResType, name: &str, causes: &[(&str, &str, &str)]) -> Response {
    let msg = causes
        .iter()
        .map(|(_, field, m)| format!("{field}: {m}"))
        .collect::<Vec<_>>();
    let message = if msg.len() == 1 {
        format!("{} \"{name}\" is invalid: {}", rt.kind, msg[0])
    } else {
        format!("{} \"{name}\" is invalid: [{}]", rt.kind, msg.join(", "))
    };
    let causes: Vec<Value> = causes
        .iter()
        .map(|(reason, field, m)| json_value!({"reason": reason, "message": m, "field": field}))
        .collect();
    let mut d = details(rt, name);
    d["kind"] = json_value!(rt.kind);
    d["causes"] = json_value!(causes);
    status_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Invalid",
        &message,
        Some(d),
    )
}

fn method_not_allowed() -> Response {
    status_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "MethodNotAllowed",
        "the server does not allow this method on the requested resource",
        None,
    )
}

fn forbidden(
    rt: &ResType,
    verb: &str,
    sub: Option<&str>,
    ns: Option<&str>,
    name: &str,
) -> Response {
    let resource = match sub {
        Some(s) => format!("{}/{s}", rt.plural),
        None => rt.plural.clone(),
    };
    let scope = match ns {
        Some(ns) => format!(" in the namespace \"{ns}\""),
        None => " at the cluster scope".into(),
    };
    let group = if rt.group.is_empty() {
        ""
    } else {
        rt.group.as_str()
    };
    status_response(
        StatusCode::FORBIDDEN,
        "Forbidden",
        &format!(
            "{} \"{name}\" is forbidden: User \"{USER}\" cannot {verb} resource \"{resource}\" in API group \"{group}\"{scope}",
            rt.plural
        ),
        Some(json_value!({"name": name, "group": group, "kind": rt.plural})),
    )
}

fn text(code: StatusCode, body: String) -> Response {
    (code, [(header::CONTENT_TYPE, "text/plain")], body).into_response()
}

// ───────────────────────────── request context ─────────────────────────────

struct Req {
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
}

impl Req {
    fn q(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn wants_table(&self) -> bool {
        self.headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("as=Table"))
    }

    fn content_type(&self) -> &str {
        self.headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
    }

    fn json_body(&self) -> Result<Value, Response> {
        if proto::is_protobuf(self.content_type(), &self.body) {
            return proto::decode(&self.body).ok_or_else(|| {
                status_response(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "UnsupportedMediaType",
                    "the body of the request was in an unknown format - accepted media types include: application/json, application/yaml",
                    None,
                )
            });
        }
        serde_json::from_slice(&self.body).map_err(|e| {
            bad_request(&format!(
                "the object provided is unrecognized (must be of type JSON): {e}"
            ))
        })
    }
}

async fn handle(
    State(world): State<SharedWorld>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let req = Req {
        method,
        path: uri.path().trim_end_matches('/').to_owned(),
        query: parse_query(uri.query()),
        headers,
        body,
    };
    // kubernetes-mcp-server's stdio transport does not forward the kubeconfig token, so
    // requests without credentials are served as the cluster's one user; a wrong token is
    // rejected like a real apiserver would.
    let auth = req
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if auth.is_some_and(|v| v != format!("Bearer {TOKEN}")) {
        return status_response(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "Unauthorized",
            None,
        );
    }
    let mut guard = world.lock().expect("world lock");
    let world = &mut *guard;
    let now = world.clock.now_unix();
    let state = world.service_mut(NAME);
    let mut k = Kube {
        types: all_types(Some(&state.store)),
        state,
        now,
    };
    k.route(&req)
}

struct Kube<'a> {
    types: Vec<ResType>,
    state: &'a mut mock_core::world::ServiceState,
    now: i64,
}

impl Kube<'_> {
    fn route(&mut self, req: &Req) -> Response {
        let path = if req.path.is_empty() {
            "/"
        } else {
            req.path.as_str()
        };
        match path {
            "/version" => return json(StatusCode::OK, version()),
            "/healthz" | "/livez" | "/readyz" => return text(StatusCode::OK, "ok".into()),
            "/api" => return json(StatusCode::OK, api_versions()),
            "/apis" => return json(StatusCode::OK, self.api_group_list()),
            _ => {}
        }
        let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match segs.as_slice() {
            ["api", "v1", rest @ ..] => self.resource_request(req, "", "v1", rest),
            ["apis", group] => match self.api_group(group) {
                Some(g) => json(StatusCode::OK, g),
                None => not_found_generic(),
            },
            ["apis", group, version, rest @ ..] => self.resource_request(req, group, version, rest),
            _ => not_found_generic(),
        }
    }

    fn api_group_list(&self) -> Value {
        let groups: Vec<Value> = groups(&self.types)
            .into_iter()
            .map(|(g, versions)| group_json(&g, &versions))
            .collect();
        json_value!({ "kind": "APIGroupList", "apiVersion": "v1", "groups": groups })
    }

    fn api_group(&self, group: &str) -> Option<Value> {
        groups(&self.types)
            .into_iter()
            .find(|(g, _)| g == group)
            .map(|(g, versions)| {
                let mut v = group_json(&g, &versions);
                v["kind"] = json_value!("APIGroup");
                v["apiVersion"] = json_value!("v1");
                v
            })
    }

    fn resource_request(
        &mut self,
        req: &Req,
        group: &str,
        version: &str,
        rest: &[&str],
    ) -> Response {
        if rest.is_empty() {
            return match resource_list(&self.types, group, version) {
                Some(list) => json(StatusCode::OK, list),
                None => not_found_generic(),
            };
        }
        let (ns, rest) = if rest[0] == "namespaces"
            && rest.len() >= 3
            && find(&self.types, group, version, rest[2]).is_some_and(|t| t.namespaced)
        {
            (Some(rest[1].to_owned()), &rest[2..])
        } else {
            (None, rest)
        };
        let Some(rt) = find(&self.types, group, version, rest[0]).cloned() else {
            return not_found_generic();
        };
        if ns.is_some() && !rt.namespaced {
            return not_found_generic();
        }
        let name = rest.get(1).map(|s| s.to_string());
        let sub = if rest.len() > 2 {
            Some(rest[2..].join("/"))
        } else {
            None
        };
        if let Some(sub) = &sub {
            let first = sub.split('/').next().unwrap_or_default();
            if !rt.has_sub(first) {
                return not_found_generic();
            }
        }
        if rt.namespaced
            && let Some(ns) = &ns
            && req.method != Method::GET
            && !self.namespace_exists(ns)
            && !(rt.plural == "namespaces")
        {
            return namespace_not_found(ns);
        }
        match (rt.group.as_str(), rt.plural.as_str()) {
            ("authorization.k8s.io", _) | ("authentication.k8s.io", _) => {
                return self.review(req, &rt, ns.as_deref());
            }
            _ => {}
        }
        if rt.group == "metrics.k8s.io" && req.method != Method::GET {
            return method_not_allowed();
        }
        match (req.method.clone(), name, sub) {
            (Method::GET, None, None) => self.list(req, &rt, ns.as_deref()),
            (Method::POST, None, None) => match ns {
                Some(ns) => self.create(req, &rt, Some(&ns)),
                None if !rt.namespaced => self.create(req, &rt, None),
                None => method_not_allowed(),
            },
            (Method::DELETE, None, None) => self.delete_collection(req, &rt, ns.as_deref()),
            (_, None, _) => method_not_allowed(),
            (m, Some(name), None) => {
                if rt.namespaced && ns.is_none() {
                    return not_found_generic();
                }
                match m {
                    Method::GET => self.get(&rt, ns.as_deref(), &name),
                    Method::PUT => self.update(req, &rt, ns.as_deref(), &name, false),
                    Method::PATCH => self.patch(req, &rt, ns.as_deref(), &name, false),
                    Method::DELETE => self.delete(req, &rt, ns.as_deref(), &name),
                    _ => method_not_allowed(),
                }
            }
            (_, Some(name), Some(sub)) => {
                if rt.namespaced && ns.is_none() {
                    return not_found_generic();
                }
                self.subresource(req, &rt, ns.as_deref(), &name, &sub)
            }
        }
    }

    // ───────────────────────────── storage ─────────────────────────────

    fn key(rt: &ResType, ns: Option<&str>, name: &str) -> String {
        match (rt.namespaced, ns) {
            (true, Some(ns)) => format!("{ns}/{name}"),
            _ => name.to_owned(),
        }
    }

    fn namespace_exists(&self, ns: &str) -> bool {
        self.state
            .store
            .collection("namespaces")
            .is_some_and(|c| c.contains(ns))
    }

    /// Stored documents of `rt` with their keys, sorted by key like etcd.
    fn stored(&self, rt: &ResType) -> Vec<(String, Value)> {
        let mut items: Vec<(String, Value)> = self
            .state
            .store
            .collection(&rt.collection())
            .map(|c| {
                c.iter()
                    .enumerate()
                    .map(|(i, (k, v))| (k.to_owned(), present(rt, k, i, v)))
                    .collect()
            })
            .unwrap_or_default();
        if rt.group.is_empty() && rt.plural == "secrets" {
            items.extend(self.helm_secrets());
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        items
    }

    fn fetch(&self, rt: &ResType, ns: Option<&str>, name: &str) -> Option<Value> {
        let key = Self::key(rt, ns, name);
        if let Some(c) = self.state.store.collection(&rt.collection())
            && let Some(i) = c.iter().position(|(k, _)| k == key)
        {
            let (k, v) = c.iter().nth(i).expect("index");
            return Some(present(rt, k, i, v));
        }
        if rt.group.is_empty() && rt.plural == "secrets" {
            return self
                .helm_secrets()
                .into_iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v);
        }
        None
    }

    fn next_rv(&mut self) -> String {
        self.state
            .ids
            .next_int("resourceVersion", 70_001)
            .to_string()
    }

    fn now_str(&self) -> String {
        format_unix(self.now)
    }

    fn save(&mut self, rt: &ResType, key: &str, obj: Value) {
        self.state
            .store
            .collection_mut(&rt.collection())
            .insert(key, obj);
    }

    fn remove(&mut self, rt: &ResType, key: &str) -> bool {
        if self
            .state
            .store
            .collection_mut(&rt.collection())
            .remove(key)
            .is_some()
        {
            return true;
        }
        if rt.group.is_empty() && rt.plural == "secrets" {
            return self.remove_helm_secret(key);
        }
        false
    }

    // ───────────────────────────── list / watch ─────────────────────────────

    fn list(&mut self, req: &Req, rt: &ResType, ns: Option<&str>) -> Response {
        let label_sel = req.q("labelSelector").unwrap_or_default();
        let field_sel = req.q("fieldSelector").unwrap_or_default();
        let labels = match parse_label_selector(label_sel) {
            Ok(l) => l,
            Err(e) => return bad_request(&format!("unable to parse requirement: {e}")),
        };
        let fields = match parse_field_selector(field_sel) {
            Ok(f) => f,
            Err(e) => return bad_request(&e),
        };
        let mut items: Vec<(String, Value)> = self
            .stored(rt)
            .into_iter()
            .filter(|(_, obj)| {
                ns.is_none_or(|ns| obj["metadata"]["namespace"].as_str() == Some(ns))
                    && labels.iter().all(|r| r.matches(&obj["metadata"]["labels"]))
                    && fields.iter().all(|f| f.matches(obj))
            })
            .collect();
        if rt.group == "metrics.k8s.io" {
            for (_, obj) in &mut items {
                if !obj["timestamp"].is_string() {
                    obj["timestamp"] = json_value!(format_unix(self.now - 15));
                }
                if !obj["window"].is_string() {
                    obj["window"] = json_value!("15.012s");
                }
            }
        }
        let list_rv = items
            .iter()
            .filter_map(|(_, o)| {
                o["metadata"]["resourceVersion"]
                    .as_str()?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(60_000)
            .to_string();

        if matches!(req.q("watch"), Some("true" | "1")) {
            let mut body = String::new();
            for (_, obj) in &items {
                body.push_str(&json_value!({"type": "ADDED", "object": obj}).to_string());
                body.push('\n');
            }
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response();
        }

        // limit / continue
        if let Some(token) = req.q("continue").filter(|t| !t.is_empty()) {
            let Some(start) = decode_continue(token) else {
                return status_response(
                    StatusCode::BAD_REQUEST,
                    "BadRequest",
                    "continue key is not valid: illegal base64 data at input byte 0",
                    None,
                );
            };
            items.retain(|(k, _)| k.as_str() >= start.as_str());
        }
        let limit = req
            .q("limit")
            .and_then(|l| l.parse::<usize>().ok())
            .unwrap_or(0);
        let mut meta = json_value!({ "resourceVersion": list_rv });
        if limit > 0 && items.len() > limit {
            let remaining = items.len() - limit;
            let next_key = items[limit].0.clone();
            items.truncate(limit);
            meta["continue"] = json_value!(encode_continue(&next_key, &list_rv));
            meta["remainingItemCount"] = json_value!(remaining);
        }
        let objs: Vec<Value> = items.into_iter().map(|(_, v)| v).collect();
        if req.wants_table() {
            return json(StatusCode::OK, table::render(rt, &objs, self.now, meta));
        }
        let objs: Vec<Value> = objs
            .into_iter()
            .map(|mut o| {
                if rt.builtin
                    && let Some(m) = o.as_object_mut()
                {
                    m.shift_remove("apiVersion");
                    m.shift_remove("kind");
                }
                o
            })
            .collect();
        json(
            StatusCode::OK,
            json_value!({
                "kind": format!("{}List", rt.kind),
                "apiVersion": rt.group_version(),
                "metadata": meta,
                "items": objs,
            }),
        )
    }

    fn get(&mut self, rt: &ResType, ns: Option<&str>, name: &str) -> Response {
        match self.fetch(rt, ns, name) {
            Some(mut obj) => {
                if rt.group == "metrics.k8s.io" && !obj["timestamp"].is_string() {
                    obj["timestamp"] = json_value!(format_unix(self.now - 15));
                    obj["window"] = json_value!("15.012s");
                }
                json(StatusCode::OK, obj)
            }
            None => not_found(rt, name),
        }
    }

    // ───────────────────────────── writes ─────────────────────────────

    fn create(&mut self, req: &Req, rt: &ResType, ns: Option<&str>) -> Response {
        let mut obj = match req.json_body() {
            Ok(v) => v,
            Err(r) => return r,
        };
        if !obj.is_object() {
            return bad_request("the object provided is unrecognized (must be of type Object)");
        }
        if let Some(kind) = obj["kind"].as_str()
            && kind != rt.kind
        {
            return bad_request(&format!(
                "the server rejected our request due to an error in our request (kind {kind} does not match {})",
                rt.kind
            ));
        }
        let body_ns = obj["metadata"]["namespace"].as_str().map(str::to_owned);
        if let (Some(b), Some(ns)) = (&body_ns, ns)
            && b != ns
        {
            return bad_request(
                "the namespace of the provided object does not match the namespace sent on the request",
            );
        }
        let name = match obj["metadata"]["name"].as_str().filter(|n| !n.is_empty()) {
            Some(n) => n.to_owned(),
            None => match obj["metadata"]["generateName"].as_str() {
                Some(prefix) => {
                    let n = self.state.ids.next_int("generateName", 0);
                    format!("{prefix}{}", name_suffix(&format!("{prefix}{n}"), 5))
                }
                None => {
                    return invalid(
                        rt,
                        "",
                        &[(
                            "FieldValueRequired",
                            "metadata.name",
                            "Required value: name or generateName is required",
                        )],
                    );
                }
            },
        };
        if let Err(msg) = validate_name(rt, &name) {
            return invalid(rt, &name, &[("FieldValueInvalid", "metadata.name", &msg)]);
        }
        let key = Self::key(rt, ns, &name);
        if self.fetch(rt, ns, &name).is_some() {
            return status_response(
                StatusCode::CONFLICT,
                "AlreadyExists",
                &format!("{} \"{name}\" already exists", rt.qualified()),
                Some(details(rt, &name)),
            );
        }
        if let Some(resp) = validate_object(rt, &name, &obj) {
            return resp;
        }
        self.initialize(rt, ns, &name, &mut obj);
        if rt.plural == "customresourcedefinitions" {
            obj["status"] = crd_status(&obj);
        }
        self.save(rt, &key, obj.clone());
        if rt.plural == "namespaces" && rt.group.is_empty() {
            self.ensure_default_sa(&name);
        }
        json(StatusCode::CREATED, obj)
    }

    /// Server-populated fields of a new object.
    fn initialize(&mut self, rt: &ResType, ns: Option<&str>, name: &str, obj: &mut Value) {
        obj["apiVersion"] = json_value!(rt.group_version());
        obj["kind"] = json_value!(rt.kind);
        let uid = self.state.ids.next_uuid("uid").to_string();
        let rv = self.next_rv();
        let now = self.now_str();
        let meta = obj["metadata"]
            .as_object_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        let mut m = Map::new();
        m.insert("name".into(), json_value!(name));
        if let Some(g) = meta.get("generateName") {
            m.insert("generateName".into(), g.clone());
        }
        if rt.namespaced
            && let Some(ns) = ns
        {
            m.insert("namespace".into(), json_value!(ns));
        }
        m.insert("uid".into(), json_value!(uid));
        m.insert("resourceVersion".into(), json_value!(rv));
        if has_generation(rt) {
            m.insert("generation".into(), json_value!(1));
        }
        m.insert("creationTimestamp".into(), json_value!(now));
        for (k, v) in meta {
            if !matches!(
                k.as_str(),
                "name"
                    | "namespace"
                    | "uid"
                    | "resourceVersion"
                    | "creationTimestamp"
                    | "generation"
                    | "generateName"
                    | "deletionTimestamp"
            ) {
                m.insert(k, v);
            }
        }
        obj["metadata"] = Value::Object(m);
        apply_defaults(rt, obj);
    }

    fn ensure_default_sa(&mut self, ns: &str) {
        let Some(sa) = find(&self.types, "", "v1", "serviceaccounts").cloned() else {
            return;
        };
        if self.fetch(&sa, Some(ns), "default").is_none() {
            let mut obj = json_value!({"metadata": {"name": "default"}});
            self.initialize(&sa, Some(ns), "default", &mut obj);
            self.save(&sa, &format!("{ns}/default"), obj);
        }
    }

    fn update(
        &mut self,
        req: &Req,
        rt: &ResType,
        ns: Option<&str>,
        name: &str,
        status_only: bool,
    ) -> Response {
        let mut incoming = match req.json_body() {
            Ok(v) => v,
            Err(r) => return r,
        };
        let Some(current) = self.fetch(rt, ns, name) else {
            return not_found(rt, name);
        };
        if let Some(n) = incoming["metadata"]["name"].as_str()
            && n != name
        {
            return bad_request(&format!(
                "the name of the object ({n}) does not match the name on the URL ({name})"
            ));
        }
        if let Some(rv) = incoming["metadata"]["resourceVersion"].as_str()
            && Some(rv) != current["metadata"]["resourceVersion"].as_str()
        {
            return conflict(rt, name);
        }
        if let Some(resp) = validate_object(rt, name, &incoming) {
            return resp;
        }
        let next = if status_only {
            let mut next = current.clone();
            next["status"] = incoming["status"].take();
            next
        } else {
            let mut next = incoming;
            if rt.has_sub("status") {
                next["status"] = current["status"].clone();
            }
            next
        };
        self.commit(rt, ns, name, &current, next)
    }

    /// Stores `next` as the new version of `current`, keeping server-owned metadata.
    fn commit(
        &mut self,
        rt: &ResType,
        ns: Option<&str>,
        name: &str,
        current: &Value,
        mut next: Value,
    ) -> Response {
        next["apiVersion"] = json_value!(rt.group_version());
        next["kind"] = json_value!(rt.kind);
        if !next["metadata"].is_object() {
            next["metadata"] = json_value!({});
        }
        for field in [
            "name",
            "namespace",
            "uid",
            "creationTimestamp",
            "generateName",
            "ownerReferences",
        ] {
            if !current["metadata"][field].is_null()
                && (field != "ownerReferences" || next["metadata"][field].is_null())
            {
                next["metadata"][field] = current["metadata"][field].clone();
            }
        }
        next["metadata"]["name"] = json_value!(name);
        if has_generation(rt) {
            let gen_now = current["metadata"]["generation"].as_i64().unwrap_or(1);
            let changed = next["spec"] != current["spec"];
            next["metadata"]["generation"] =
                json_value!(if changed { gen_now + 1 } else { gen_now });
        }
        if next == *current {
            return json(StatusCode::OK, next);
        }
        next["metadata"]["resourceVersion"] = json_value!(self.next_rv());
        let key = Self::key(rt, ns, name);
        if rt.group.is_empty() && rt.plural == "secrets" && self.is_helm_virtual(&key) {
            self.remove_helm_secret(&key);
        }
        self.save(rt, &key, next.clone());
        json(StatusCode::OK, next)
    }

    fn patch(
        &mut self,
        req: &Req,
        rt: &ResType,
        ns: Option<&str>,
        name: &str,
        status_only: bool,
    ) -> Response {
        let ct = req.content_type().to_owned();
        let patch = match req.json_body() {
            Ok(v) => v,
            Err(r) => return r,
        };
        let current = self.fetch(rt, ns, name);
        let is_apply = ct.starts_with("application/apply-patch");
        let Some(current) = current else {
            if is_apply && !status_only {
                return self.apply_create(rt, ns, name, patch);
            }
            return not_found(rt, name);
        };
        let mut next = current.clone();
        if ct.starts_with("application/json-patch+json") {
            if let Err(e) = json_patch(&mut next, &patch) {
                return status_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Invalid",
                    &e,
                    Some(details(rt, name)),
                );
            }
        } else if ct.starts_with("application/merge-patch+json") {
            if !patch.is_object() {
                return status_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Invalid",
                    "merge patch must be a JSON object",
                    Some(details(rt, name)),
                );
            }
            merge_patch(&mut next, &patch);
        } else if ct.starts_with("application/strategic-merge-patch+json") {
            if !rt.builtin {
                return status_response(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "UnsupportedMediaType",
                    "the body of the request was in an unknown format - accepted media types include: application/json-patch+json, application/merge-patch+json, application/apply-patch+yaml",
                    None,
                );
            }
            strategic_merge(&mut next, &patch);
        } else if is_apply {
            if patch["metadata"]["name"]
                .as_str()
                .is_some_and(|n| n != name)
            {
                return bad_request("the name of the object does not match the name on the URL");
            }
            let mut patch = patch;
            if let Some(m) = patch.as_object_mut() {
                m.shift_remove("status");
            }
            strategic_merge(&mut next, &patch);
        } else {
            return status_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UnsupportedMediaType",
                &format!(
                    "the body of the request was in an unknown format - accepted media types include: application/json-patch+json, application/merge-patch+json, application/apply-patch+yaml, application/strategic-merge-patch+json; got {ct}"
                ),
                None,
            );
        }
        if status_only {
            let mut only = current.clone();
            only["status"] = next["status"].take();
            next = only;
        } else if rt.has_sub("status") {
            next["status"] = current["status"].clone();
        }
        if let Some(resp) = validate_object(rt, name, &next) {
            return resp;
        }
        self.commit(rt, ns, name, &current, next)
    }

    fn apply_create(
        &mut self,
        rt: &ResType,
        ns: Option<&str>,
        name: &str,
        mut obj: Value,
    ) -> Response {
        if let Some(m) = obj.as_object_mut() {
            m.shift_remove("status");
        }
        if obj["metadata"]["name"].as_str().is_some_and(|n| n != name) {
            return bad_request("the name of the object does not match the name on the URL");
        }
        if let Err(msg) = validate_name(rt, name) {
            return invalid(rt, name, &[("FieldValueInvalid", "metadata.name", &msg)]);
        }
        if let Some(resp) = validate_object(rt, name, &obj) {
            return resp;
        }
        self.initialize(rt, ns, name, &mut obj);
        if rt.plural == "customresourcedefinitions" {
            obj["status"] = crd_status(&obj);
        }
        let key = Self::key(rt, ns, name);
        self.save(rt, &key, obj.clone());
        if rt.plural == "namespaces" && rt.group.is_empty() {
            self.ensure_default_sa(name);
        }
        json(StatusCode::CREATED, obj)
    }

    fn delete(&mut self, req: &Req, rt: &ResType, ns: Option<&str>, name: &str) -> Response {
        if rt.plural == "namespaces"
            && rt.group.is_empty()
            && matches!(name, "default" | "kube-system" | "kube-public")
        {
            return status_response(
                StatusCode::FORBIDDEN,
                "Forbidden",
                &format!("namespaces \"{name}\" is forbidden: this namespace may not be deleted"),
                Some(json_value!({"name": name, "kind": "namespaces"})),
            );
        }
        let Some(obj) = self.fetch(rt, ns, name) else {
            return not_found(rt, name);
        };
        let propagation = req
            .json_body()
            .ok()
            .and_then(|b| b["propagationPolicy"].as_str().map(str::to_owned))
            .or_else(|| req.q("propagationPolicy").map(str::to_owned))
            .unwrap_or_else(|| "Background".into());
        let key = Self::key(rt, ns, name);
        self.remove(rt, &key);
        self.cascade(rt, ns, &obj, &propagation);
        let mut out = obj;
        if rt.plural == "namespaces" && rt.group.is_empty() {
            out["metadata"]["deletionTimestamp"] = json_value!(self.now_str());
            out["status"] = json_value!({"phase": "Terminating"});
        }
        if rt.plural == "pods" && rt.group.is_empty() {
            out["metadata"]["deletionTimestamp"] = json_value!(format_unix(self.now + 30));
            out["metadata"]["deletionGracePeriodSeconds"] = json_value!(30);
        }
        json(StatusCode::OK, out)
    }

    /// Garbage collection and controller reactions to a deletion.
    fn cascade(&mut self, rt: &ResType, ns: Option<&str>, obj: &Value, propagation: &str) {
        let name = obj["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if rt.group.is_empty() && rt.plural == "namespaces" {
            let types = self.types.clone();
            for t in types.iter().filter(|t| t.namespaced) {
                let prefix = format!("{name}/");
                let keys: Vec<String> = self
                    .state
                    .store
                    .collection(&t.collection())
                    .map(|c| {
                        c.iter()
                            .map(|(k, _)| k.to_owned())
                            .filter(|k| k.starts_with(&prefix))
                            .collect()
                    })
                    .unwrap_or_default();
                for k in keys {
                    self.state.store.collection_mut(&t.collection()).remove(&k);
                }
            }
            let helm_keys: Vec<String> = self
                .state
                .store
                .collection(HELM_COLLECTION)
                .map(|c| {
                    c.iter()
                        .map(|(k, _)| k.to_owned())
                        .filter(|k| k.starts_with(&format!("{name}/")))
                        .collect()
                })
                .unwrap_or_default();
            for k in helm_keys {
                self.state.store.collection_mut(HELM_COLLECTION).remove(&k);
            }
            return;
        }
        if propagation == "Orphan" {
            return;
        }
        // Owned objects: Deployment -> ReplicaSet -> Pod, Job/StatefulSet/DaemonSet -> Pod.
        let owned: &[(&str, &str)] = match (rt.group.as_str(), rt.plural.as_str()) {
            ("apps", "deployments") => &[("apps", "replicasets")],
            ("apps", "replicasets")
            | ("apps", "statefulsets")
            | ("apps", "daemonsets")
            | ("batch", "jobs")
            | ("", "replicationcontrollers") => &[("", "pods")],
            ("batch", "cronjobs") => &[("batch", "jobs")],
            _ => &[],
        };
        for (g, plural) in owned {
            let Some(child_rt) = self
                .types
                .iter()
                .find(|t| t.group == *g && t.plural == *plural && t.namespaced)
                .cloned()
            else {
                continue;
            };
            let children: Vec<Value> = self
                .stored(&child_rt)
                .into_iter()
                .map(|(_, v)| v)
                .filter(|c| {
                    c["metadata"]["namespace"].as_str() == ns && owned_by(c, &rt.kind, &name)
                })
                .collect();
            for child in children {
                let cname = child["metadata"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                self.remove(&child_rt, &Self::key(&child_rt, ns, &cname));
                self.cascade(&child_rt, ns, &child, propagation);
            }
        }
        // A ReplicaSet replaces a deleted pod it still wants.
        if rt.group.is_empty() && rt.plural == "pods" {
            self.replace_pod(ns, obj);
        }
    }

    fn replace_pod(&mut self, ns: Option<&str>, pod: &Value) {
        let Some(owner) = pod["metadata"]["ownerReferences"].as_array().and_then(|o| {
            o.iter()
                .find(|r| r["kind"] == "ReplicaSet" && r["controller"] != false)
        }) else {
            return;
        };
        let Some(rs_name) = owner["name"].as_str() else {
            return;
        };
        let Some(rs_rt) = find(&self.types, "apps", "v1", "replicasets").cloned() else {
            return;
        };
        let Some(rs) = self.fetch(&rs_rt, ns, rs_name) else {
            return;
        };
        let Some(pod_rt) = find(&self.types, "", "v1", "pods").cloned() else {
            return;
        };
        let desired = rs["spec"]["replicas"].as_i64().unwrap_or(1);
        let current = self
            .stored(&pod_rt)
            .into_iter()
            .filter(|(_, p)| {
                p["metadata"]["namespace"].as_str() == ns && owned_by(p, "ReplicaSet", rs_name)
            })
            .count() as i64;
        if current >= desired {
            return;
        }
        let n = self.state.ids.next_int("pod-replacement", 0);
        let new_name = format!("{rs_name}-{}", name_suffix(&format!("{rs_name}/{n}"), 5));
        let template = &rs["spec"]["template"];
        let mut spec = template["spec"].clone();
        if let Some(node) = pod["spec"]["nodeName"].as_str() {
            spec["nodeName"] = json_value!(node);
        }
        let containers = spec["containers"].as_array().cloned().unwrap_or_default();
        let statuses: Vec<Value> = containers
            .iter()
            .map(|c| {
                json_value!({
                    "name": c["name"],
                    "image": c["image"],
                    "imageID": "",
                    "ready": false,
                    "restartCount": 0,
                    "started": false,
                    "state": {"waiting": {"reason": "ContainerCreating"}},
                    "lastState": {},
                })
            })
            .collect();
        let mut new_pod = json_value!({
            "metadata": {
                "name": new_name,
                "generateName": format!("{rs_name}-"),
                "labels": template["metadata"]["labels"],
                "ownerReferences": pod["metadata"]["ownerReferences"],
            },
            "spec": spec,
            "status": {
                "phase": "Pending",
                "conditions": [
                    {"type": "PodScheduled", "status": "True", "lastProbeTime": null, "lastTransitionTime": self.now_str()},
                    {"type": "Ready", "status": "False", "lastProbeTime": null, "lastTransitionTime": self.now_str(), "reason": "ContainersNotReady", "message": "containers with unready status"}
                ],
                "hostIP": pod["status"]["hostIP"],
                "qosClass": pod["status"]["qosClass"],
                "startTime": self.now_str(),
                "containerStatuses": statuses,
            },
        });
        if let Some(a) = template["metadata"]["annotations"].as_object() {
            new_pod["metadata"]["annotations"] = json_value!(a);
        }
        self.initialize(&pod_rt, ns, &new_name, &mut new_pod);
        let key = Self::key(&pod_rt, ns, &new_name);
        self.save(&pod_rt, &key, new_pod);
    }

    fn delete_collection(&mut self, req: &Req, rt: &ResType, ns: Option<&str>) -> Response {
        let labels = match parse_label_selector(req.q("labelSelector").unwrap_or_default()) {
            Ok(l) => l,
            Err(e) => return bad_request(&format!("unable to parse requirement: {e}")),
        };
        let fields = match parse_field_selector(req.q("fieldSelector").unwrap_or_default()) {
            Ok(f) => f,
            Err(e) => return bad_request(&e),
        };
        let victims: Vec<Value> = self
            .stored(rt)
            .into_iter()
            .map(|(_, v)| v)
            .filter(|obj| {
                ns.is_none_or(|ns| obj["metadata"]["namespace"].as_str() == Some(ns))
                    && labels.iter().all(|r| r.matches(&obj["metadata"]["labels"]))
                    && fields.iter().all(|f| f.matches(obj))
            })
            .collect();
        for obj in &victims {
            let name = obj["metadata"]["name"].as_str().unwrap_or_default();
            let ons = obj["metadata"]["namespace"].as_str();
            self.remove(rt, &Self::key(rt, ons, name));
            self.cascade(rt, ons, obj, "Background");
        }
        let items: Vec<Value> = victims;
        json(
            StatusCode::OK,
            json_value!({
                "kind": format!("{}List", rt.kind),
                "apiVersion": rt.group_version(),
                "metadata": {"resourceVersion": self.next_rv()},
                "items": items,
            }),
        )
    }

    // ───────────────────────────── subresources ─────────────────────────────

    fn subresource(
        &mut self,
        req: &Req,
        rt: &ResType,
        ns: Option<&str>,
        name: &str,
        sub: &str,
    ) -> Response {
        let first = sub.split('/').next().unwrap_or_default();
        match first {
            "status" => match req.method {
                Method::GET => self.get(rt, ns, name),
                Method::PUT => self.update(req, rt, ns, name, true),
                Method::PATCH => self.patch(req, rt, ns, name, true),
                _ => method_not_allowed(),
            },
            "scale" => self.scale(req, rt, ns, name),
            "log" if req.method == Method::GET => self.pod_log(req, rt, ns, name),
            "exec" | "attach" | "portforward" => {
                if self.fetch(rt, ns, name).is_none() {
                    return not_found(rt, name);
                }
                let verb = if req.method == Method::GET {
                    "get"
                } else {
                    "create"
                };
                forbidden(rt, verb, Some(first), ns, name)
            }
            "eviction" if req.method == Method::POST => {
                let Some(obj) = self.fetch(rt, ns, name) else {
                    return not_found(rt, name);
                };
                let key = Self::key(rt, ns, name);
                self.remove(rt, &key);
                self.cascade(rt, ns, &obj, "Background");
                json(
                    StatusCode::CREATED,
                    json_value!({"kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Success", "code": 201}),
                )
            }
            "proxy" => self.proxy(
                req,
                rt,
                ns,
                name,
                sub.strip_prefix("proxy")
                    .unwrap_or_default()
                    .trim_start_matches('/'),
            ),
            "token" if req.method == Method::POST => {
                if self.fetch(rt, ns, name).is_none() {
                    return not_found(rt, name);
                }
                let body = req.json_body().unwrap_or(json_value!({}));
                let seconds = body["spec"]["expirationSeconds"].as_i64().unwrap_or(3600);
                let token = format!(
                    "eyJhbGciOiJSUzI1NiIsImtpZCI6InRvb2xiZW5jaCJ9.{}.{}",
                    b64_encode(
                        format!(
                            "{{\"sub\":\"system:serviceaccount:{}:{name}\"}}",
                            ns.unwrap_or_default()
                        )
                        .as_bytes()
                    )
                    .trim_end_matches('='),
                    name_suffix(&format!("token/{name}/{}", self.now), 43)
                );
                json(
                    StatusCode::CREATED,
                    json_value!({
                        "kind": "TokenRequest",
                        "apiVersion": "authentication.k8s.io/v1",
                        "metadata": {"name": name, "namespace": ns, "creationTimestamp": null},
                        "spec": {"audiences": ["https://kubernetes.default.svc.cluster.local"], "expirationSeconds": seconds, "boundObjectRef": null},
                        "status": {"token": token, "expirationTimestamp": format_unix(self.now + seconds)},
                    }),
                )
            }
            "finalize" if req.method == Method::PUT => self.update(req, rt, ns, name, false),
            _ => {
                if self.fetch(rt, ns, name).is_none() {
                    return not_found(rt, name);
                }
                method_not_allowed()
            }
        }
    }

    fn scale(&mut self, req: &Req, rt: &ResType, ns: Option<&str>, name: &str) -> Response {
        let Some(obj) = self.fetch(rt, ns, name) else {
            return not_found(rt, name);
        };
        let scale_of = |o: &Value| {
            let selector = o["spec"]["selector"]["matchLabels"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .or_else(|| {
                    o["spec"]["selector"].as_object().map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|v| format!("{k}={v}")))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                })
                .unwrap_or_default();
            let mut status =
                json_value!({"replicas": o["status"]["replicas"].as_i64().unwrap_or(0)});
            if !selector.is_empty() {
                status["selector"] = json_value!(selector);
            }
            json_value!({
                "kind": "Scale",
                "apiVersion": "autoscaling/v1",
                "metadata": {
                    "name": o["metadata"]["name"],
                    "namespace": o["metadata"]["namespace"],
                    "uid": o["metadata"]["uid"],
                    "resourceVersion": o["metadata"]["resourceVersion"],
                    "creationTimestamp": o["metadata"]["creationTimestamp"],
                },
                "spec": {"replicas": o["spec"]["replicas"].as_i64().unwrap_or(1)},
                "status": status,
            })
        };
        let current_scale = scale_of(&obj);
        let replicas = match req.method {
            Method::GET => return json(StatusCode::OK, current_scale),
            Method::PUT => {
                let body = match req.json_body() {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                if let Some(rv) = body["metadata"]["resourceVersion"].as_str()
                    && Some(rv) != obj["metadata"]["resourceVersion"].as_str()
                {
                    return conflict(rt, name);
                }
                body["spec"]["replicas"].clone()
            }
            Method::PATCH => {
                let patch = match req.json_body() {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let mut s = current_scale.clone();
                if req
                    .content_type()
                    .starts_with("application/json-patch+json")
                {
                    if let Err(e) = json_patch(&mut s, &patch) {
                        return status_response(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Invalid",
                            &e,
                            None,
                        );
                    }
                } else {
                    merge_patch(&mut s, &patch);
                }
                s["spec"]["replicas"].clone()
            }
            _ => return method_not_allowed(),
        };
        let Some(n) = replicas.as_i64().filter(|n| *n >= 0) else {
            let mut d = details(rt, name);
            d["kind"] = json_value!("Scale");
            return status_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid",
                &format!(
                    "Scale.autoscaling \"{name}\" is invalid: spec.replicas: Invalid value: {replicas}: must be greater than or equal to 0"
                ),
                Some(d),
            );
        };
        let mut next = obj.clone();
        next["spec"]["replicas"] = json_value!(n);
        let resp = self.commit(rt, ns, name, &obj, next);
        if resp.status() != StatusCode::OK {
            return resp;
        }
        let updated = self.fetch(rt, ns, name).unwrap_or(obj);
        json(StatusCode::OK, scale_of(&updated))
    }

    fn pod_log(&mut self, req: &Req, rt: &ResType, ns: Option<&str>, name: &str) -> Response {
        let Some(pod) = self.fetch(rt, ns, name) else {
            return not_found(rt, name);
        };
        let containers: Vec<String> = pod["spec"]["containers"]
            .as_array()
            .map(|c| {
                c.iter()
                    .filter_map(|c| c["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let init: Vec<String> = pod["spec"]["initContainers"]
            .as_array()
            .map(|c| {
                c.iter()
                    .filter_map(|c| c["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let container = match req.q("container").filter(|c| !c.is_empty()) {
            Some(c) => c.to_owned(),
            None if containers.len() == 1 => containers[0].clone(),
            None => {
                return bad_request(&format!(
                    "a container name must be specified for pod {name}, choose one of: [{}]",
                    containers.join(" ")
                ));
            }
        };
        if !containers.contains(&container) && !init.contains(&container) {
            return bad_request(&format!(
                "container {container} is not valid for pod {name}"
            ));
        }
        let previous = matches!(req.q("previous"), Some("true" | "1"));
        let cstatus = pod["status"]["containerStatuses"]
            .as_array()
            .into_iter()
            .chain(pod["status"]["initContainerStatuses"].as_array())
            .flatten()
            .find(|c| c["name"] == container.as_str())
            .cloned()
            .unwrap_or(Value::Null);
        let logs = self
            .state
            .store
            .collection("pod_logs")
            .and_then(|c| c.get(&format!("{}/{name}/{container}", ns.unwrap_or_default())))
            .cloned()
            .unwrap_or(Value::Null);
        let lines: Vec<String> = if previous {
            if !cstatus["lastState"]["terminated"].is_object() && logs["previous"].is_null() {
                return bad_request(&format!(
                    "previous terminated container \"{container}\" in pod \"{name}\" not found"
                ));
            }
            str_lines(&logs["previous"])
        } else {
            if let Some(reason) = cstatus["state"]["waiting"]["reason"].as_str()
                && matches!(
                    reason,
                    "ContainerCreating"
                        | "PodInitializing"
                        | "ImagePullBackOff"
                        | "ErrImagePull"
                        | "CreateContainerConfigError"
                )
            {
                return bad_request(&format!(
                    "container \"{container}\" in pod \"{name}\" is waiting to start: {}",
                    if reason == "ContainerCreating" || reason == "PodInitializing" {
                        reason.to_owned()
                    } else {
                        "trying and failing to pull image".to_string()
                    }
                ));
            }
            str_lines(&logs["current"])
        };
        let mut lines = lines;
        if let Some(t) = req.q("tailLines").and_then(|t| t.parse::<usize>().ok())
            && lines.len() > t
        {
            lines = lines.split_off(lines.len() - t);
        }
        if !matches!(req.q("timestamps"), Some("true" | "1")) {
            lines = lines
                .into_iter()
                .map(|l| match l.split_once(' ') {
                    Some((ts, rest))
                        if ts.len() >= 20
                            && ts.as_bytes().get(4) == Some(&b'-')
                            && ts.contains('T')
                            && logs["timestamped"] == true =>
                    {
                        rest.to_owned()
                    }
                    _ => l,
                })
                .collect();
        }
        let mut out = lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        text(StatusCode::OK, out)
    }

    fn proxy(
        &mut self,
        req: &Req,
        rt: &ResType,
        ns: Option<&str>,
        name: &str,
        path: &str,
    ) -> Response {
        let Some(obj) = self.fetch(rt, ns, name) else {
            return not_found(rt, name);
        };
        match rt.plural.as_str() {
            "nodes" => match path {
                "stats/summary" => {
                    let stats = self
                        .state
                        .store
                        .collection("node_stats")
                        .and_then(|c| c.get(name))
                        .cloned()
                        .unwrap_or_else(|| synthesize_node_stats(&obj, self.now));
                    json(StatusCode::OK, stats)
                }
                "logs" | "logs/" => {
                    let query = req.q("query").unwrap_or_default();
                    let lines = self
                        .state
                        .store
                        .collection("node_logs")
                        .and_then(|c| c.get(&format!("{name}/{query}")))
                        .map(|d| str_lines(&d["lines"]));
                    let Some(mut lines) = lines else {
                        return text(
                            StatusCode::NOT_FOUND,
                            format!("query: \"{query}\" not found: file does not exist\n"),
                        );
                    };
                    if let Some(t) = req.q("tailLines").and_then(|t| t.parse::<usize>().ok())
                        && lines.len() > t
                    {
                        lines = lines.split_off(lines.len() - t);
                    }
                    text(StatusCode::OK, lines.join("\n") + "\n")
                }
                "healthz" => text(StatusCode::OK, "ok".into()),
                "configz" => json(
                    StatusCode::OK,
                    json_value!({"kubeletconfig": {"enableServer": true, "staticPodPath": "/etc/kubernetes/manifests", "syncFrequency": "1m0s", "authentication": {"anonymous": {"enabled": false}, "webhook": {"enabled": true, "cacheTTL": "2m0s"}}, "authorization": {"mode": "Webhook"}, "clusterDomain": "cluster.local", "clusterDNS": ["10.96.0.10"], "maxPods": 110, "podPidsLimit": -1, "cgroupDriver": "systemd", "evictionHard": {"imagefs.available": "15%", "memory.available": "100Mi", "nodefs.available": "10%", "nodefs.inodesFree": "5%"}}}),
                ),
                _ => text(StatusCode::NOT_FOUND, "404 page not found\n".into()),
            },
            "services" => status_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailable",
                &format!("no endpoints available for service \"{name}\""),
                None,
            ),
            _ => {
                let ip = obj["status"]["podIP"].as_str().unwrap_or("10.244.0.1");
                status_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ServiceUnavailable",
                    &format!(
                        "error trying to reach service: dial tcp {ip}:80: connect: connection refused"
                    ),
                    None,
                )
            }
        }
    }

    // ───────────────────────────── authz ─────────────────────────────

    fn review(&mut self, req: &Req, rt: &ResType, ns: Option<&str>) -> Response {
        if req.method != Method::POST {
            return method_not_allowed();
        }
        let mut body = match req.json_body() {
            Ok(v) => v,
            Err(r) => return r,
        };
        body["apiVersion"] = json_value!(rt.group_version());
        body["kind"] = json_value!(rt.kind);
        body["metadata"] = json_value!({"creationTimestamp": null});
        match rt.plural.as_str() {
            "selfsubjectaccessreviews" | "subjectaccessreviews" | "localsubjectaccessreviews" => {
                let attrs = &body["spec"]["resourceAttributes"];
                let denied = attrs["resource"] == "pods"
                    && matches!(
                        attrs["subresource"].as_str(),
                        Some("exec" | "attach" | "portforward")
                    );
                body["status"] = if denied {
                    json_value!({"allowed": false, "reason": format!("RBAC: no rule allows {} on pods/{} for User \"{USER}\"", attrs["verb"].as_str().unwrap_or("create"), attrs["subresource"].as_str().unwrap_or_default())})
                } else {
                    json_value!({"allowed": true, "reason": format!("RBAC: allowed by ClusterRoleBinding \"toolbench-ops\" of ClusterRole \"toolbench-ops\" to User \"{USER}\"")})
                };
                if let Some(ns) = ns {
                    body["metadata"]["namespace"] = json_value!(ns);
                }
            }
            "selfsubjectrulesreviews" => {
                body["status"] = json_value!({
                    "resourceRules": [
                        {"verbs": ["*"], "apiGroups": ["*"], "resources": ["*"]},
                        {"verbs": ["create"], "apiGroups": ["authorization.k8s.io"], "resources": ["selfsubjectaccessreviews", "selfsubjectrulesreviews"]}
                    ],
                    "nonResourceRules": [
                        {"verbs": ["get"], "nonResourceURLs": ["/api", "/api/*", "/apis", "/apis/*", "/healthz", "/livez", "/readyz", "/version", "/version/"]}
                    ],
                    "incomplete": false
                });
            }
            "selfsubjectreviews" => {
                body["status"] = json_value!({"userInfo": {"username": USER, "uid": stable_uid("user/toolbench"), "groups": ["system:authenticated", "octo:sre"]}});
            }
            "tokenreviews" => {
                let ok = body["spec"]["token"] == TOKEN;
                body["status"] = if ok {
                    json_value!({"authenticated": true, "user": {"username": USER, "uid": stable_uid("user/toolbench"), "groups": ["system:authenticated", "octo:sre"]}, "audiences": ["https://kubernetes.default.svc.cluster.local"]})
                } else {
                    json_value!({"authenticated": false, "user": {}, "error": "[invalid bearer token, token lookup failed]"})
                };
            }
            _ => return method_not_allowed(),
        }
        json(StatusCode::CREATED, body)
    }

    // ───────────────────────────── helm ─────────────────────────────

    /// Helm releases from `helm_releases`, as the Secrets Helm's storage driver writes.
    fn helm_secrets(&self) -> Vec<(String, Value)> {
        let Some(c) = self.state.store.collection(HELM_COLLECTION) else {
            return vec![];
        };
        c.iter()
            .map(|(key, rel)| {
                let ns = rel["namespace"].as_str().unwrap_or("default");
                let name = rel["name"].as_str().unwrap_or_default();
                let version = rel["version"].as_i64().unwrap_or(1);
                let status = rel["info"]["status"].as_str().unwrap_or("deployed");
                let deployed = rel["info"]["last_deployed"]
                    .as_str()
                    .unwrap_or(DEFAULT_CREATED);
                let modified = util::parse_rfc3339(deployed).unwrap_or(0);
                let secret_name = format!("{HELM_PREFIX}{name}.v{version}");
                let inner = b64_encode(rel.to_string().as_bytes());
                let secret = json_value!({
                    "apiVersion": "v1",
                    "kind": "Secret",
                    "metadata": {
                        "name": secret_name,
                        "namespace": ns,
                        "uid": stable_uid(&format!("helm/{key}")),
                        "resourceVersion": (50_000 + util::fnv(key) % 9_000).to_string(),
                        "creationTimestamp": deployed,
                        "labels": {
                            "modifiedAt": modified.to_string(),
                            "name": name,
                            "owner": "helm",
                            "status": status,
                            "version": version.to_string(),
                        },
                    },
                    "type": "helm.sh/release.v1",
                    "data": { "release": b64_encode(inner.as_bytes()) },
                });
                (format!("{ns}/{secret_name}"), secret)
            })
            .collect()
    }

    fn helm_key_for_secret(&self, secret_key: &str) -> Option<String> {
        let (ns, secret_name) = secret_key.split_once('/')?;
        let rest = secret_name.strip_prefix(HELM_PREFIX)?;
        let c = self.state.store.collection(HELM_COLLECTION)?;
        c.iter()
            .find(|(_, rel)| {
                rel["namespace"].as_str().unwrap_or("default") == ns
                    && format!(
                        "{}.v{}",
                        rel["name"].as_str().unwrap_or_default(),
                        rel["version"].as_i64().unwrap_or(1)
                    ) == rest
            })
            .map(|(k, _)| k.to_owned())
    }

    fn is_helm_virtual(&self, secret_key: &str) -> bool {
        self.helm_key_for_secret(secret_key).is_some()
    }

    fn remove_helm_secret(&mut self, secret_key: &str) -> bool {
        match self.helm_key_for_secret(secret_key) {
            Some(k) => self
                .state
                .store
                .collection_mut(HELM_COLLECTION)
                .remove(&k)
                .is_some(),
            None => false,
        }
    }
}

fn conflict(rt: &ResType, name: &str) -> Response {
    status_response(
        StatusCode::CONFLICT,
        "Conflict",
        &format!(
            "Operation cannot be fulfilled on {} \"{name}\": the object has been modified; please apply your changes to the latest version and try again",
            rt.qualified()
        ),
        Some(details(rt, name)),
    )
}

fn str_lines(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|l| l.as_str().map(str::to_owned))
            .collect(),
        Value::String(s) => s.lines().map(str::to_owned).collect(),
        _ => vec![],
    }
}

fn owned_by(obj: &Value, kind: &str, name: &str) -> bool {
    obj["metadata"]["ownerReferences"]
        .as_array()
        .is_some_and(|refs| refs.iter().any(|r| r["kind"] == kind && r["name"] == name))
}

fn has_generation(rt: &ResType) -> bool {
    !(rt.group.is_empty()
        && matches!(
            rt.plural.as_str(),
            "pods"
                | "configmaps"
                | "secrets"
                | "events"
                | "namespaces"
                | "nodes"
                | "serviceaccounts"
                | "endpoints"
                | "services"
                | "persistentvolumeclaims"
                | "persistentvolumes"
        ))
}

fn validate_name(rt: &ResType, name: &str) -> Result<(), String> {
    let bytes = name.as_bytes();
    let subdomain = !name.is_empty()
        && name.len() <= 253
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'.')
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric();
    let label_only = rt.group.is_empty() && matches!(rt.plural.as_str(), "namespaces" | "services");
    if !subdomain {
        return Err(format!(
            "Invalid value: \"{name}\": a lowercase RFC 1123 subdomain must consist of lower case alphanumeric characters, '-' or '.', and must start and end with an alphanumeric character (e.g. 'example.com', regex used for validation is '[a-z0-9]([-a-z0-9]*[a-z0-9])?(\\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*')"
        ));
    }
    if label_only && (name.contains('.') || name.len() > 63) {
        return Err(format!(
            "Invalid value: \"{name}\": a lowercase RFC 1123 label must consist of lower case alphanumeric characters or '-', and must start and end with an alphanumeric character (e.g. 'my-name',  or '123-abc', regex used for validation is '[a-z0-9]([-a-z0-9]*[a-z0-9])?')"
        ));
    }
    Ok(())
}

/// The few validations agents are likely to trip.
fn validate_object(rt: &ResType, name: &str, obj: &Value) -> Option<Response> {
    let containers_required = |spec: &Value, field: &str| {
        if spec["containers"].as_array().is_none_or(Vec::is_empty) {
            Some(invalid(
                rt,
                name,
                &[("FieldValueRequired", field, "Required value")],
            ))
        } else {
            for (i, c) in spec["containers"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                if c["image"].as_str().is_none_or(str::is_empty) {
                    let f = field.replace("containers", &format!("containers[{i}].image"));
                    return Some(invalid(
                        rt,
                        name,
                        &[("FieldValueRequired", &f, "Required value")],
                    ));
                }
            }
            None
        }
    };
    match (rt.group.as_str(), rt.plural.as_str()) {
        ("", "pods") => containers_required(&obj["spec"], "spec.containers"),
        ("apps", "deployments" | "statefulsets" | "replicasets" | "daemonsets") => {
            if let Some(r) = obj["spec"]["replicas"].as_i64()
                && r < 0
            {
                let v = format!("Invalid value: {r}: must be greater than or equal to 0");
                return Some(invalid(
                    rt,
                    name,
                    &[("FieldValueInvalid", "spec.replicas", &v)],
                ));
            }
            containers_required(
                &obj["spec"]["template"]["spec"],
                "spec.template.spec.containers",
            )
        }
        ("", "configmaps") => {
            if let Some(data) = obj["data"].as_object() {
                for (k, v) in data {
                    if !v.is_string() {
                        let _ = k;
                        return Some(bad_request(
                            "ConfigMap in version \"v1\" cannot be handled as a ConfigMap: json: cannot unmarshal number into Go struct field ConfigMap.data of type string",
                        ));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn apply_defaults(rt: &ResType, obj: &mut Value) {
    match (rt.group.as_str(), rt.plural.as_str()) {
        ("", "namespaces") => {
            let name = obj["metadata"]["name"].clone();
            obj["metadata"]["labels"]["kubernetes.io/metadata.name"] = name;
            obj["spec"] = json_value!({"finalizers": ["kubernetes"]});
            obj["status"] = json_value!({"phase": "Active"});
        }
        ("", "pods") => {
            if obj["status"].is_null() {
                obj["status"] = json_value!({"phase": "Pending", "qosClass": "BestEffort"});
            }
            if obj["spec"]["restartPolicy"].is_null() {
                obj["spec"]["restartPolicy"] = json_value!("Always");
            }
            if obj["spec"]["serviceAccountName"].is_null() {
                obj["spec"]["serviceAccountName"] = json_value!("default");
            }
        }
        ("", "services") => {
            if obj["spec"]["type"].is_null() {
                obj["spec"]["type"] = json_value!("ClusterIP");
            }
            if obj["spec"]["clusterIP"].is_null() {
                let h = util::fnv(obj["metadata"]["name"].as_str().unwrap_or_default());
                obj["spec"]["clusterIP"] =
                    json_value!(format!("10.96.{}.{}", (h >> 8) % 250 + 1, h % 250 + 2));
            }
            if obj["status"].is_null() {
                obj["status"] = json_value!({"loadBalancer": {}});
            }
        }
        ("apps", "deployments") => {
            if obj["spec"]["replicas"].is_null() {
                obj["spec"]["replicas"] = json_value!(1);
            }
            if obj["spec"]["strategy"].is_null() {
                obj["spec"]["strategy"] = json_value!({"type": "RollingUpdate", "rollingUpdate": {"maxUnavailable": "25%", "maxSurge": "25%"}});
            }
            if obj["status"].is_null() {
                obj["status"] = json_value!({});
            }
        }
        ("", "secrets") => {
            if obj["type"].is_null() {
                obj["type"] = json_value!("Opaque");
            }
            if let Some(string_data) = obj
                .get("stringData")
                .cloned()
                .and_then(|v| v.as_object().cloned())
            {
                for (k, v) in string_data {
                    obj["data"][&k] =
                        json_value!(b64_encode(v.as_str().unwrap_or_default().as_bytes()));
                }
                obj.as_object_mut()
                    .expect("object")
                    .shift_remove("stringData");
            }
        }
        _ => {}
    }
}

fn crd_status(crd: &Value) -> Value {
    json_value!({
        "conditions": [
            {"type": "NamesAccepted", "status": "True", "reason": "NoConflicts", "message": "no conflicts found"},
            {"type": "Established", "status": "True", "reason": "InitialNamesAccepted", "message": "the initial names have been accepted"}
        ],
        "acceptedNames": crd["spec"]["names"],
        "storedVersions": crd["spec"]["versions"].as_array().map(|v| v.iter().filter(|v| v["storage"] == true).map(|v| v["name"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
    })
}

/// A stored document as the API serves it: type meta plus server-populated metadata for
/// dataset documents that omit it.
fn present(rt: &ResType, key: &str, index: usize, doc: &Value) -> Value {
    let mut obj = doc.clone();
    if !obj.is_object() {
        return obj;
    }
    let (ns, name) = match key.split_once('/') {
        Some((ns, name)) if rt.namespaced => (Some(ns), name),
        _ => (None, key),
    };
    let mut meta = obj["metadata"].as_object().cloned().unwrap_or_default();
    let mut m = Map::new();
    m.insert(
        "name".into(),
        meta.shift_remove("name").unwrap_or(json_value!(name)),
    );
    if let Some(g) = meta.shift_remove("generateName") {
        m.insert("generateName".into(), g);
    }
    if let Some(ns) = ns {
        m.insert(
            "namespace".into(),
            meta.shift_remove("namespace").unwrap_or(json_value!(ns)),
        );
    }
    m.insert(
        "uid".into(),
        meta.shift_remove("uid")
            .unwrap_or_else(|| json_value!(stable_uid(&format!("{}/{key}", rt.collection())))),
    );
    m.insert(
        "resourceVersion".into(),
        meta.shift_remove("resourceVersion").unwrap_or_else(|| {
            json_value!((50_000 + index as u64 * 7 + util::fnv(&rt.collection()) % 7).to_string())
        }),
    );
    if has_generation(rt) {
        m.insert(
            "generation".into(),
            meta.shift_remove("generation").unwrap_or(json_value!(1)),
        );
    }
    m.insert(
        "creationTimestamp".into(),
        meta.shift_remove("creationTimestamp")
            .unwrap_or(json_value!(DEFAULT_CREATED)),
    );
    for (k, v) in meta {
        m.insert(k, v);
    }
    if rt.group.is_empty() && rt.plural == "namespaces" {
        let labels = m.entry("labels").or_insert_with(|| json_value!({}));
        if labels.get("kubernetes.io/metadata.name").is_none() {
            labels["kubernetes.io/metadata.name"] = json_value!(name);
        }
    }
    let mut out = Map::new();
    out.insert("apiVersion".into(), json_value!(rt.group_version()));
    out.insert("kind".into(), json_value!(rt.kind));
    out.insert("metadata".into(), Value::Object(m));
    for (k, v) in obj.as_object_mut().expect("object").iter_mut() {
        if !matches!(k.as_str(), "apiVersion" | "kind" | "metadata") {
            out.insert(k.clone(), v.take());
        }
    }
    if rt.group.is_empty() && rt.plural == "namespaces" && out.get("status").is_none() {
        out.insert("spec".into(), json_value!({"finalizers": ["kubernetes"]}));
        out.insert("status".into(), json_value!({"phase": "Active"}));
    }
    Value::Object(out)
}

fn group_json(group: &str, versions: &[String]) -> Value {
    let vs: Vec<Value> = versions
        .iter()
        .map(|v| json_value!({"groupVersion": format!("{group}/{v}"), "version": v}))
        .collect();
    json_value!({
        "name": group,
        "versions": vs,
        "preferredVersion": {"groupVersion": format!("{group}/{}", versions[0]), "version": versions[0]},
    })
}

fn version() -> Value {
    json_value!({
        "major": "1",
        "minor": "33",
        "emulationMajor": "1",
        "emulationMinor": "33",
        "minCompatibilityMajor": "1",
        "minCompatibilityMinor": "32",
        "gitVersion": "v1.33.4",
        "gitCommit": "74cdb426e0d6a5ebe44a0cd49d8b4c29e2ea7aa0",
        "gitTreeState": "clean",
        "buildDate": "2025-08-12T18:00:42Z",
        "goVersion": "go1.24.5",
        "compiler": "gc",
        "platform": "linux/amd64"
    })
}

fn api_versions() -> Value {
    json_value!({
        "kind": "APIVersions",
        "versions": ["v1"],
        "serverAddressByClientCIDRs": [{"clientCIDR": "0.0.0.0/0", "serverAddress": "10.0.12.4:6443"}]
    })
}

fn encode_continue(next_key: &str, rv: &str) -> String {
    let token = json_value!({"v": "meta.k8s.io/v1", "rv": rv.parse::<u64>().unwrap_or(0), "start": format!("{next_key}\u{0}")});
    b64_encode(token.to_string().as_bytes())
}

fn decode_continue(token: &str) -> Option<String> {
    let bytes = b64_decode(token)?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    Some(v["start"].as_str()?.trim_end_matches('\u{0}').to_owned())
}

fn synthesize_node_stats(node: &Value, now: i64) -> Value {
    let name = node["metadata"]["name"].as_str().unwrap_or_default();
    let ts = format_unix(now - 10);
    json_value!({
        "node": {
            "nodeName": name,
            "systemContainers": [],
            "startTime": node["metadata"]["creationTimestamp"],
            "cpu": {"time": ts, "usageNanoCores": 412_000_000_u64, "usageCoreNanoSeconds": 9_812_331_000_000_u64},
            "memory": {"time": ts, "availableBytes": 9_663_676_416_u64, "usageBytes": 6_442_450_944_u64, "workingSetBytes": 5_368_709_120_u64, "rssBytes": 3_221_225_472_u64, "pageFaults": 112_331, "majorPageFaults": 12},
            "fs": {"time": ts, "availableBytes": 64_424_509_440_u64, "capacityBytes": 107_374_182_400_u64, "usedBytes": 42_949_672_960_u64, "inodesFree": 6_291_456, "inodes": 6_553_600, "inodesUsed": 262_144}
        },
        "pods": []
    })
}

// ───────────────────────────── selectors ─────────────────────────────

#[derive(Debug)]
enum LabelReq {
    Eq(String, String),
    NotEq(String, String),
    In(String, Vec<String>),
    NotIn(String, Vec<String>),
    Exists(String),
    NotExists(String),
}

impl LabelReq {
    fn matches(&self, labels: &Value) -> bool {
        let get = |k: &str| labels.get(k).and_then(Value::as_str);
        match self {
            Self::Eq(k, v) => get(k) == Some(v.as_str()),
            Self::NotEq(k, v) => get(k) != Some(v.as_str()),
            Self::In(k, vs) => get(k).is_some_and(|x| vs.iter().any(|v| v == x)),
            Self::NotIn(k, vs) => get(k).is_none_or(|x| !vs.iter().any(|v| v == x)),
            Self::Exists(k) => get(k).is_some(),
            Self::NotExists(k) => get(k).is_none(),
        }
    }
}

fn split_top_level(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

fn parse_label_selector(s: &str) -> Result<Vec<LabelReq>, String> {
    let mut out = Vec::new();
    for part in split_top_level(s) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let set = |rest: &str| -> Result<Vec<String>, String> {
            let inner = rest
                .trim()
                .strip_prefix('(')
                .and_then(|r| r.strip_suffix(')'))
                .ok_or_else(|| format!("found '{rest}', expected: '('"))?;
            Ok(inner
                .split(',')
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .collect())
        };
        if let Some((k, rest)) = p.split_once(" notin ") {
            out.push(LabelReq::NotIn(k.trim().into(), set(rest)?));
        } else if let Some((k, rest)) = p.split_once(" in ") {
            out.push(LabelReq::In(k.trim().into(), set(rest)?));
        } else if let Some((k, v)) = p.split_once("!=") {
            out.push(LabelReq::NotEq(k.trim().into(), v.trim().into()));
        } else if let Some((k, v)) = p.split_once("==") {
            out.push(LabelReq::Eq(k.trim().into(), v.trim().into()));
        } else if let Some((k, v)) = p.split_once('=') {
            out.push(LabelReq::Eq(k.trim().into(), v.trim().into()));
        } else if let Some(k) = p.strip_prefix('!') {
            out.push(LabelReq::NotExists(k.trim().into()));
        } else if p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
        {
            out.push(LabelReq::Exists(p.into()));
        } else {
            return Err(format!("{p}: invalid label selector"));
        }
    }
    Ok(out)
}

#[derive(Debug)]
struct FieldReq {
    path: String,
    value: String,
    negate: bool,
}

impl FieldReq {
    fn matches(&self, obj: &Value) -> bool {
        let mut cur = obj;
        for part in self.path.split('.') {
            cur = &cur[part];
        }
        let actual = match cur {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        (actual == self.value) != self.negate
    }
}

fn parse_field_selector(s: &str) -> Result<Vec<FieldReq>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let (path, value, negate) = if let Some((k, v)) = p.split_once("!=") {
            (k, v, true)
        } else if let Some((k, v)) = p.split_once("==") {
            (k, v, false)
        } else if let Some((k, v)) = p.split_once('=') {
            (k, v, false)
        } else {
            return Err(format!("invalid selector: '{p}'; can't understand '{p}'"));
        };
        out.push(FieldReq {
            path: path.trim().into(),
            value: value.trim().into(),
            negate,
        });
    }
    Ok(out)
}
