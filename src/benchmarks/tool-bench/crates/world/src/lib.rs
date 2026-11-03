//! Hosts every mock service plus the admin API for one scenario.

pub mod grade;
pub mod replay;
pub mod tls;

use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, SocketAddr},
};

use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use axum_server::tls_rustls::RustlsConfig;
use mock_core::{
    admin::admin_app,
    serve::{serve, service_app},
    service::MockService,
    world::SharedWorld,
};
use tokio::{net::TcpListener, task::JoinSet};

pub const ADMIN_PORT: u16 = 9000;
pub const TLS_PORT: u16 = 443;

/// Every mock service compiled into the world.
pub fn services() -> Vec<Box<dyn MockService>> {
    vec![
        Box::new(mock_argocd::Argocd),
        Box::new(mock_aws::Aws),
        Box::new(mock_brave::Brave),
        Box::new(mock_cloudflare::Cloudflare),
        Box::new(mock_datadog::Datadog),
        Box::new(mock_elasticsearch::Elasticsearch),
        Box::new(mock_exa::Exa),
        Box::new(mock_firecrawl::Firecrawl),
        Box::new(mock_github::GitHub),
        Box::new(mock_gitlab::Gitlab),
        Box::new(mock_google::Google),
        Box::new(mock_grafana::Grafana),
        Box::new(mock_hubspot::Hubspot),
        Box::new(mock_jira::Jira),
        Box::new(mock_kubernetes::Kubernetes),
        Box::new(mock_linear::Linear),
        Box::new(mock_mongodb::Mongodb),
        Box::new(mock_notion::Notion),
        Box::new(mock_okta::Okta),
        Box::new(mock_pagerduty::Pagerduty),
        Box::new(mock_salesforce::Salesforce),
        Box::new(mock_sentry::Sentry),
        Box::new(mock_shopify::Shopify),
        Box::new(mock_slack::Slack),
        Box::new(mock_stripe::Stripe),
        Box::new(mock_supabase::Supabase),
        Box::new(mock_tavily::Tavily),
        Box::new(mock_terraform::Terraform),
        Box::new(mock_vercel::Vercel),
        Box::new(mock_zendesk::Zendesk),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortChoice {
    /// Each service on its own fixed port, admin on [`ADMIN_PORT`] (container use).
    Fixed,
    /// OS-assigned ports (tests).
    Ephemeral,
}

pub struct Running {
    pub admin: SocketAddr,
    /// HTTPS listener that dispatches by `Host` to the service owning that hostname.
    pub tls: SocketAddr,
    /// PEM of the CA that signed the TLS certificate (also served at `/_bench/ca.pem`).
    pub ca_pem: String,
    pub services: BTreeMap<&'static str, SocketAddr>,
    tasks: JoinSet<io::Result<()>>,
}

impl Running {
    /// Runs until any listener stops, returning its error.
    pub async fn wait(mut self) -> io::Result<()> {
        while let Some(result) = self.tasks.join_next().await {
            result.map_err(io::Error::other)??;
        }
        Ok(())
    }
}

/// Admin routes the tools container and health checks reach without the admin token.
const OPEN_ADMIN_PATHS: [&str; 2] = ["/_bench/health", "/_bench/ca.pem"];

/// Binds the admin API and every service, then serves them in the background.
pub async fn spawn(world: SharedWorld, ip: IpAddr, ports: PortChoice) -> io::Result<Running> {
    spawn_guarded(world, ip, ports, None).await
}

/// [`spawn`], with every admin route except [`OPEN_ADMIN_PATHS`] requiring
/// `Authorization: Bearer <admin_token>` when a token is set. Trials set one so the agent,
/// which shares the network with the world, cannot read the state or call the grader.
pub async fn spawn_guarded(
    world: SharedWorld,
    ip: IpAddr,
    ports: PortChoice,
    admin_token: Option<String>,
) -> io::Result<Running> {
    let port = |fixed: u16| match ports {
        PortChoice::Fixed => fixed,
        PortChoice::Ephemeral => 0,
    };
    let mut tasks = JoinSet::new();
    let services = services();
    let ca = tls::LocalCa::generate(&tls::all_hosts(&services)).map_err(io::Error::other)?;

    let admin_listener = TcpListener::bind((ip, port(ADMIN_PORT))).await?;
    let admin = admin_listener.local_addr()?;
    let ca_pem = ca.ca_pem.clone();
    let admin_routes = admin_app(world.clone())
        .merge(grade::routes(world.clone()))
        .merge(Router::new().route(
            "/_bench/ca.pem",
            get(move || {
                let pem = ca_pem.clone();
                async move { pem }
            }),
        ));
    let admin_routes = match admin_token {
        Some(token) => admin_routes.layer(middleware::from_fn(move |request, next| {
            require_admin_token(token.clone(), request, next)
        })),
        None => admin_routes,
    };
    tasks.spawn(serve(admin_listener, admin_routes));

    let tls_listener = std::net::TcpListener::bind((ip, port(TLS_PORT)))?;
    tls_listener.set_nonblocking(true)?;
    let tls = tls_listener.local_addr()?;
    let config = RustlsConfig::from_pem(
        ca.leaf_cert_pem.clone().into_bytes(),
        ca.leaf_key_pem.clone().into_bytes(),
    )
    .await?;
    let tls_app = tls::host_router(world.clone(), &services);
    tasks.spawn(async move {
        axum_server::from_tcp_rustls(tls_listener, config)?
            .serve(tls_app.into_make_service_with_connect_info::<SocketAddr>())
            .await
    });

    let mut bound = BTreeMap::new();
    for service in &services {
        let listener = TcpListener::bind((ip, port(service.port()))).await?;
        bound.insert(service.name(), listener.local_addr()?);
        tasks.spawn(serve(
            listener,
            service_app(world.clone(), service.as_ref()),
        ));
    }

    Ok(Running {
        admin,
        tls,
        ca_pem: ca.ca_pem,
        services: bound,
        tasks,
    })
}

async fn require_admin_token(token: String, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let authorized = OPEN_ADMIN_PATHS.contains(&path)
        || request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|presented| presented == token);
    if authorized {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
