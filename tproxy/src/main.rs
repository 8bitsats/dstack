use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use config::{Config, TlsConfig};
use main_service::{Proxy, RpcHandler};
use ra_rpc::{client::RaClient, rocket_helper::QuoteVerifier};
use rocket::fairing::AdHoc;
use tracing::info;

mod config;
mod main_service;
mod models;
mod proxy;
mod web_routes;

fn app_version() -> String {
    const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
    const VERSION: &str = git_version::git_version!(
        args = ["--abbrev=20", "--always", "--dirty=-modified"],
        prefix = "git:",
        fallback = "unknown"
    );
    format!("v{CARGO_PKG_VERSION} ({VERSION})")
}

#[derive(Parser)]
#[command(author, version, about, long_version = app_version())]
struct Args {
    /// Path to the configuration file
    #[arg(short, long)]
    config: Option<String>,
}

#[cfg(unix)]
fn set_max_ulimit() -> Result<()> {
    use nix::sys::resource::{getrlimit, setrlimit, Resource};
    let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE)?;
    if soft < hard {
        setrlimit(Resource::RLIMIT_NOFILE, hard, hard)?;
    }
    Ok(())
}

async fn maybe_gen_certs(config: &Config, tls_config: &TlsConfig) -> Result<()> {
    if config.gen_certs_for.is_empty() {
        return Ok(());
    }
    let kms_url = config.kms_url.clone();
    if kms_url.is_empty() {
        bail!("kms_url is required when gen_certs turned on");
    }
    let kms_url = format!("{kms_url}/prpc");
    info!("Getting CA cert from {kms_url}");
    let client = RaClient::new(kms_url, true).context("Failed to create kms client")?;
    let client = kms_rpc::kms_client::KmsClient::new(client);
    let ca_cert = client.get_meta().await?.ca_cert;
    let key = ra_tls::rcgen::KeyPair::generate().context("Failed to generate key")?;
    let cert = ra_tls::cert::CertRequest::builder()
        .key(&key)
        .subject("tproxy")
        .alt_names(&[config.gen_certs_for.clone()])
        .usage_server_auth(true)
        .build()
        .self_signed()
        .context("Failed to self-sign rpc cert")?;

    write_cert(&tls_config.mutual.ca_certs, &ca_cert)?;
    write_cert(&tls_config.certs, &cert.pem())?;
    write_cert(&tls_config.key, &key.serialize_pem())?;
    Ok(())
}

fn write_cert(path: &str, cert: &str) -> Result<()> {
    info!("Writing cert to file: {path}");
    safe_write::safe_write(path, cert)?;
    Ok(())
}

#[rocket::main]
async fn main() -> Result<()> {
    {
        use tracing_subscriber::{fmt, EnvFilter};
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).init();
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    let args = Args::parse();
    let figment = config::load_config_figment(args.config.as_deref());

    let config = figment.focus("core").extract::<Config>()?;
    config::setup_wireguard(&config.wg)?;

    let tls_config = figment.focus("tls").extract::<TlsConfig>()?;
    maybe_gen_certs(&config, &tls_config)
        .await
        .context("Failed to generate certs")?;

    #[cfg(unix)]
    if config.set_ulimit {
        set_max_ulimit()?;
    }

    let proxy_config = config.proxy.clone();
    let pccs_url = config.pccs_url.clone();
    let state = main_service::Proxy::new(config)?;
    state.lock().reconfigure()?;
    proxy::start(proxy_config, state.clone());

    let mut rocket = rocket::custom(figment)
        .mount("/", web_routes::routes())
        .mount("/prpc", ra_rpc::prpc_routes!(Proxy, RpcHandler))
        .attach(AdHoc::on_response("Add app version header", |_req, res| {
            Box::pin(async move {
                res.set_raw_header("X-App-Version", app_version());
            })
        }))
        .manage(state);
    let verifier = QuoteVerifier::new(pccs_url);
    rocket = rocket.manage(verifier);
    rocket
        .launch()
        .await
        .map_err(|err| anyhow!(err.to_string()))?;
    Ok(())
}
