use std::sync::Arc;

use anyhow::{bail, Context, Result};
use fs_err as fs;
use k256::ecdsa::SigningKey;
use kms_rpc::{
    kms_server::{KmsRpc, KmsServer},
    AppId, AppKeyResponse, GetAppKeyRequest, GetMetaResponse, GetTempCaCertResponse,
    KmsKeyResponse, KmsKeys, PublicKeyResponse,
};
use ra_rpc::{CallContext, RpcCall};
use ra_tls::{
    attestation::VerifiedAttestation as Attestation,
    cert::{CaCert, CertRequest},
    kdf,
};
use upgrade_authority::BootInfo;

use crate::{config::KmsConfig, crypto::derive_k256_key};

mod upgrade_authority;

#[derive(Clone)]
pub struct KmsState {
    inner: Arc<KmsStateInner>,
}

impl std::ops::Deref for KmsState {
    type Target = KmsStateInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub struct KmsStateInner {
    config: KmsConfig,
    root_ca: CaCert,
    k256_key: SigningKey,
    temp_ca_cert: String,
    temp_ca_key: String,
}

impl KmsState {
    pub fn new(config: KmsConfig) -> Result<Self> {
        let root_ca = CaCert::load(&config.root_ca_cert, &config.root_ca_key)
            .context("Failed to load root CA certificate")?;
        let key_bytes = fs::read(&config.k256_key).context("Failed to read ECDSA root key")?;
        let k256_key =
            SigningKey::from_slice(&key_bytes).context("Failed to load ECDSA root key")?;
        let temp_ca_key =
            fs::read_to_string(&config.tmp_ca_key).context("Faeild to read temp ca key")?;
        let temp_ca_cert =
            fs::read_to_string(&config.tmp_ca_cert).context("Faeild to read temp ca cert")?;
        Ok(Self {
            inner: Arc::new(KmsStateInner {
                config,
                root_ca,
                k256_key,
                temp_ca_cert,
                temp_ca_key,
            }),
        })
    }
}

pub struct RpcHandler {
    state: KmsState,
    attestation: Option<Attestation>,
}

struct BootConfig {
    boot_info: BootInfo,
    tproxy_app_id: String,
}

impl RpcHandler {
    fn ensure_attested(&self) -> Result<&Attestation> {
        let Some(attestation) = &self.attestation else {
            bail!("No attestation provided");
        };
        Ok(attestation)
    }

    async fn ensure_kms_allowed(&self) -> Result<BootInfo> {
        self.ensure_app_allowed(true).await.map(|b| b.boot_info)
    }

    async fn ensure_app_allowed(&self, is_kms: bool) -> Result<BootConfig> {
        let att = self.ensure_attested()?;
        let report = att
            .report
            .report
            .as_td10()
            .context("Failed to decode TD report")?;
        let app_info = att.decode_app_info()?;
        let boot_info = BootInfo {
            mrtd: report.mr_td.to_vec(),
            rtmr0: report.rt_mr0.to_vec(),
            rtmr1: report.rt_mr1.to_vec(),
            rtmr2: report.rt_mr2.to_vec(),
            rtmr3: report.rt_mr3.to_vec(),
            mr_enclave: app_info.mr_enclave.to_vec(),
            mr_image: app_info.mr_image.to_vec(),
            mr_key_provider: app_info.mr_key_provider.to_vec(),
            app_id: app_info.app_id,
            compose_hash: app_info.compose_hash,
            instance_id: app_info.instance_id,
            device_id: app_info.device_id,
            key_provider_info: app_info.key_provider_info,
            event_log: String::from_utf8(att.raw_event_log.clone())
                .context("Failed to serialize event log")?,
        };
        let response = self
            .state
            .config
            .auth_api
            .is_app_allowed(&boot_info, is_kms)
            .await?;
        if !response.is_allowed {
            bail!("Boot denied: {}", response.reason);
        }
        Ok(BootConfig {
            boot_info,
            tproxy_app_id: response.tproxy_app_id,
        })
    }
}

impl KmsRpc for RpcHandler {
    async fn get_app_key(self, _request: GetAppKeyRequest) -> Result<AppKeyResponse> {
        let attest = self.ensure_attested()?;
        let BootConfig {
            boot_info,
            tproxy_app_id,
        } = self
            .ensure_app_allowed(false)
            .await
            .context("App not allowed")?;
        let app_id = boot_info.app_id;
        let instance_id = boot_info.instance_id;

        let app_key = kdf::derive_ecdsa_key_pair(
            &self.state.root_ca.key,
            &[&app_id[..], "app-key".as_bytes()],
        )
        .context("Failed to derive app key")?;

        let context_data = vec![&app_id[..], &instance_id[..], b"app-disk-crypt-key"];
        let app_disk_key = kdf::derive_dh_secret(&self.state.root_ca.key, &context_data)
            .context("Failed to derive app disk key")?;
        let env_crypt_key = {
            let secret =
                kdf::derive_dh_secret(&self.state.root_ca.key, &[&app_id[..], b"env-encrypt-key"])
                    .context("Failed to derive env encrypt key")?;
            let secret = x25519_dalek::StaticSecret::from(secret);
            secret.to_bytes()
        };
        let app_id_str = hex::encode(&app_id);
        let subject = format!("{app_id_str}{}", self.state.config.subject_postfix);
        let req = CertRequest::builder()
            .subject(&subject)
            .ca_level(1)
            .quote(&attest.quote)
            .event_log(&attest.raw_event_log)
            .key(&app_key)
            .build();

        let cert = self
            .state
            .root_ca
            .sign(req)
            .context("Failed to sign certificate")?
            .pem();

        let (k256_key, k256_signature) = {
            let (k256_app_key, signature, recid) = derive_k256_key(
                &self.state.k256_key,
                &app_id,
                &[&app_id[..], "app-key".as_bytes()],
            )
            .context("Failed to derive app ecdsa key")?;

            let mut signature = signature.to_vec();
            signature.push(recid.to_byte());
            (k256_app_key.to_bytes().to_vec(), signature)
        };

        Ok(AppKeyResponse {
            disk_crypt_key: app_disk_key.to_vec(),
            env_crypt_key: env_crypt_key.to_vec(),
            app_key: app_key.serialize_pem(),
            certificate_chain: vec![cert, self.state.root_ca.cert.pem()],
            k256_key,
            k256_signature,
            tproxy_app_id,
        })
    }

    async fn get_app_env_encrypt_pub_key(self, request: AppId) -> Result<PublicKeyResponse> {
        let secret = kdf::derive_dh_secret(
            &self.state.root_ca.key,
            &[&request.app_id[..], "env-encrypt-key".as_bytes()],
        )
        .context("Failed to derive env encrypt key")?;
        let secret = x25519_dalek::StaticSecret::from(secret);
        let pubkey = x25519_dalek::PublicKey::from(&secret);
        Ok(PublicKeyResponse {
            public_key: pubkey.to_bytes().to_vec(),
        })
    }

    async fn get_meta(self) -> Result<GetMetaResponse> {
        Ok(GetMetaResponse {
            ca_cert: self.state.inner.root_ca.cert.pem(),
            allow_any_upgrade: self.state.inner.config.auth_api.is_dev(),
            k256_pubkey: self.state.inner.k256_key.to_bytes().to_vec(),
        })
    }

    async fn get_kms_key(self) -> Result<KmsKeyResponse> {
        if self.state.config.onboard.quote_enabled {
            let _info = self.ensure_kms_allowed().await?;
        }
        Ok(KmsKeyResponse {
            tmp_ca_key: self.state.inner.temp_ca_key.clone(),
            keys: vec![KmsKeys {
                ca_key: self.state.inner.root_ca.key.serialize_pem(),
                k256_key: self.state.inner.k256_key.to_bytes().to_vec(),
            }],
        })
    }

    async fn get_temp_ca_cert(self) -> Result<GetTempCaCertResponse> {
        Ok(GetTempCaCertResponse {
            temp_ca_cert: self.state.inner.temp_ca_cert.clone(),
            temp_ca_key: self.state.inner.temp_ca_key.clone(),
        })
    }
}

impl RpcCall<KmsState> for RpcHandler {
    type PrpcService = KmsServer<Self>;

    fn construct(context: CallContext<'_, KmsState>) -> Result<Self> {
        Ok(RpcHandler {
            state: context.state.clone(),
            attestation: context.attestation,
        })
    }
}

pub fn rpc_methods() -> &'static [&'static str] {
    <KmsServer<RpcHandler>>::supported_methods()
}
