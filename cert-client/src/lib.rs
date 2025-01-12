use anyhow::{Context, Result};
use dstack_types::{AppKeys, KeyProvider};
use kms_rpc::{kms_client::KmsClient, SignCertRequest};
use ra_rpc::client::{RaClient, RaClientConfig};
use ra_tls::{
    attestation::QuoteContentType,
    cert::{CaCert, CertConfig, CertSigningRequest},
    rcgen::KeyPair,
};
use tdx_attest::{eventlog::read_event_logs, get_quote};

pub enum CertRequestClient {
    Local { ca: CaCert },
    Kms { client: KmsClient<RaClient> },
}

impl CertRequestClient {
    pub async fn sign_csr(
        &self,
        csr: &CertSigningRequest,
        signature: &[u8],
    ) -> Result<Vec<String>> {
        match self {
            CertRequestClient::Local { ca } => {
                let cert = ca
                    .sign_csr(csr, None)
                    .context("Failed to sign certificate")?;
                Ok(vec![cert.pem(), ca.pem_cert.clone()])
            }
            CertRequestClient::Kms { client } => {
                let response = client
                    .sign_cert(SignCertRequest {
                        csr: csr.to_vec(),
                        signature: signature.to_vec(),
                    })
                    .await?;
                Ok(response.certificate_chain)
            }
        }
    }

    pub async fn create(keys: &AppKeys, pccs_url: Option<&str>) -> Result<CertRequestClient> {
        match &keys.key_provider {
            KeyProvider::Local { key } => {
                let ca = CaCert::new(keys.ca_cert.clone(), key.clone())
                    .context("Failed to create CA")?;
                Ok(CertRequestClient::Local { ca })
            }
            KeyProvider::Kms { url } => {
                let tmp_client =
                    RaClient::new(url.into(), true).context("Failed to create RA client")?;
                let tmp_client = KmsClient::new(tmp_client);
                let tmp_cert = tmp_client
                    .get_temp_ca_cert()
                    .await
                    .context("Failed to get RA cert")?;

                let ra_client = RaClientConfig::builder()
                    .remote_uri(url.clone())
                    .tls_client_cert(tmp_cert.temp_ca_cert)
                    .tls_client_key(tmp_cert.temp_ca_key)
                    .tls_ca_cert(keys.ca_cert.clone())
                    .tls_built_in_root_certs(false)
                    .maybe_pccs_url(pccs_url.map(|s| s.to_string()))
                    .build()
                    .into_client()
                    .context("Failed to create RA client")?;
                let client = KmsClient::new(ra_client);
                Ok(CertRequestClient::Kms { client })
            }
        }
    }

    pub async fn request_cert(&self, key: &KeyPair, config: CertConfig) -> Result<Vec<String>> {
        let pubkey = key.public_key_der();
        let report_data = QuoteContentType::RaTlsCert.to_report_data(&pubkey);
        let (_, quote) = get_quote(&report_data, None).context("Failed to get quote")?;
        let event_log = read_event_logs().context("Failed to decode event log")?;
        let event_log = serde_json::to_vec(&event_log).context("Failed to serialize event log")?;

        let csr = CertSigningRequest {
            confirm: "please sign cert:".to_string(),
            pubkey,
            config,
            quote,
            event_log,
        };
        let signature = csr.signed_by(key).context("Failed to sign the CSR")?;
        self.sign_csr(&csr, &signature)
            .await
            .context("Failed to sign the CSR")
    }
}
