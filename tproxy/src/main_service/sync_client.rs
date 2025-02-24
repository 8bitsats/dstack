use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use ra_rpc::client::{RaClient, RaClientConfig};
use tappd_rpc::DeriveKeyArgs;
use tproxy_rpc::{tproxy_client::TproxyClient, TproxyState};
use tracing::{error, info};

use crate::{config::Config, tappd_client};

use super::{ProxyNodeInfo, ProxyState};

struct SyncClient {
    in_tapp: bool,
    cert_pem: String,
    key_pem: String,
    ca_cert_pem: String,
    app_id: Vec<u8>,
    timeout: Duration,
}

impl SyncClient {
    fn create_rpc_client(&self, url: &str) -> Result<TproxyClient<RaClient>> {
        let app_id = self.app_id.clone();
        let url = format!("{}/prpc", url.trim_end_matches('/'));
        let client = if self.in_tapp {
            RaClientConfig::builder()
                .remote_uri(url)
                // Don't verify server RA because we use the CA cert from KMS to verify
                // the server cert.
                .verify_server_attestation(false)
                .tls_no_check(true)
                .tls_no_check_hostname(false)
                .tls_client_cert(self.cert_pem.clone())
                .tls_client_key(self.key_pem.clone())
                .tls_ca_cert(self.ca_cert_pem.clone())
                .tls_built_in_root_certs(false)
                .cert_validator(Box::new(move |cert| {
                    let cert = cert.context("TLS cert not found")?;
                    let remote_app_id = cert.app_id.context("App id not found")?;
                    if remote_app_id != app_id {
                        return Err(anyhow::anyhow!("Remote app id mismatch"));
                    }
                    Ok(())
                }))
                .build()
                .into_client()
                .context("failed to create client")?
        } else {
            RaClient::new(url, true)?
        };
        Ok(TproxyClient::new(client))
    }

    async fn sync_state(&self, url: &str, state: &TproxyState) -> Result<()> {
        info!("Trying to sync state to {url}");
        let rpc = self.create_rpc_client(url)?;
        tokio::time::timeout(self.timeout, rpc.update_state(state.clone()))
            .await
            .ok()
            .context("Timeout while syncing state")?
            .context("Failed to sync state")?;
        info!("Synced state to {url}");
        Ok(())
    }

    async fn sync_state_ignore_error(&self, url: &str, state: &TproxyState) -> bool {
        match self.sync_state(url, state).await {
            Ok(_) => true,
            Err(e) => {
                error!("Failed to sync state to {url}: {e:?}");
                false
            }
        }
    }
}

pub(crate) async fn sync_task(proxy: Weak<Mutex<ProxyState>>, config: Arc<Config>) -> Result<()> {
    let sync_client = if config.run_as_tapp {
        let tappd_client = tappd_client().context("Failed to create tappd_client")?;
        let keys = tappd_client
            .derive_key(DeriveKeyArgs {
                path: "/sync-state-client".into(),
                subject: "".into(),
                alt_names: vec![],
                usage_ra_tls: false,
                usage_server_auth: false,
                usage_client_auth: true,
                random_seed: true,
            })
            .await
            .context("Failed to get sync-client keys")?;
        let my_app_id = tappd_client
            .info()
            .await
            .context("Failed to get tappd info")?
            .app_id;
        SyncClient {
            in_tapp: true,
            cert_pem: keys.certificate_chain.join("\n"),
            key_pem: keys.key,
            ca_cert_pem: keys.certificate_chain.last().cloned().unwrap_or_default(),
            app_id: my_app_id,
            timeout: config.sync.timeout,
        }
    } else {
        SyncClient {
            in_tapp: false,
            cert_pem: "".into(),
            key_pem: "".into(),
            ca_cert_pem: "".into(),
            app_id: vec![],
            timeout: config.sync.timeout,
        }
    };

    let mut last_broadcast_time = Instant::now();

    loop {
        let broadcast = last_broadcast_time.elapsed() >= config.sync.broadcast_interval;
        if broadcast {
            last_broadcast_time = Instant::now();
        }

        let Some(proxy) = proxy.upgrade() else {
            info!("Proxy state was dropped, stopping sync task");
            break;
        };

        let (mut nodes, apps) = proxy.lock().unwrap().dump_state();
        // Dedup nodes by URL, keeping the latest one
        let mut url_map = BTreeMap::<String, ProxyNodeInfo>::new();
        for node in nodes {
            match url_map.get(&node.url) {
                Some(existing) if existing.last_seen >= node.last_seen => {}
                _ => {
                    url_map.insert(node.url.clone(), node);
                }
            }
        }
        nodes = url_map.into_values().collect();
        // Sort nodes by pubkey
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let self_idx = nodes
            .iter()
            .position(|n| n.wg_peer.pk == config.wg.public_key)
            .unwrap_or(0);

        let state = TproxyState {
            nodes: nodes.into_iter().map(|n| n.into()).collect(),
            apps: apps.into_iter().map(|a| a.into()).collect(),
        };

        if state.nodes.is_empty() {
            // If no nodes exist yet, sync with bootnode
            sync_client
                .sync_state_ignore_error(&config.sync.bootnode, &state)
                .await;
        } else {
            let nodes = &state.nodes;
            // Try nodes after self, wrapping around to beginning
            let mut success = false;
            for i in 1..nodes.len() {
                let idx = (self_idx + i) % nodes.len();
                if sync_client
                    .sync_state_ignore_error(&nodes[idx].url, &state)
                    .await
                {
                    success = true;
                    if !broadcast {
                        break;
                    }
                }
            }

            // If no node succeeded, try bootnode as fallback
            if !success {
                info!("Fallback to sync with bootnode");
                sync_client
                    .sync_state_ignore_error(&config.sync.bootnode, &state)
                    .await;
            }
        }

        tokio::time::sleep(config.sync.interval).await;
    }
    Ok(())
}
