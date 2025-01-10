#![allow(async_fn_in_trait)]

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use prpc::{
    codec::encode_message_to_vec,
    server::{ProtoError, Service as PrpcService},
};
use tracing::{error, info};

pub use ra_tls::attestation::{Attestation, VerifiedAttestation};

#[cfg(feature = "rocket")]
pub mod rocket_helper;

#[cfg(feature = "client")]
pub mod client;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEndpoint {
    Tcp(SocketAddr),
    Quic(SocketAddr),
    Unix(PathBuf),
    Vsock { cid: u32, port: u32 },
    Other(String),
}

#[derive(Clone, bon::Builder)]
pub struct CallContext<'a, State> {
    pub state: &'a State,
    pub attestation: Option<VerifiedAttestation>,
    pub remote_endpoint: Option<RemoteEndpoint>,
}

pub trait RpcCall<State>: Sized {
    type PrpcService: PrpcService + From<Self> + Send + 'static;

    fn construct(context: CallContext<'_, State>) -> Result<Self>;

    async fn call(
        self,
        method: String,
        payload: Vec<u8>,
        is_json: bool,
        is_query: bool,
    ) -> (u16, Vec<u8>) {
        dispatch_prpc(
            method,
            payload,
            is_json,
            is_query,
            <Self::PrpcService as From<Self>>::from(self),
        )
        .await
    }
}

async fn dispatch_prpc(
    path: String,
    data: Vec<u8>,
    json: bool,
    query: bool,
    server: impl PrpcService + Send + 'static,
) -> (u16, Vec<u8>) {
    use prpc::server::Error;

    info!("dispatching request: {}", path);
    let result = server.dispatch_request(&path, data, json, query).await;
    let (code, data) = match result {
        Ok(data) => (200, data),
        Err(err) => {
            error!("rpc error: {:?}", err);
            let (code, error) = match err {
                Error::NotFound => (404, "method Not Found".to_string()),
                Error::DecodeError(err) => (400, format!("DecodeError({err:?})")),
                Error::BadRequest(msg) => (400, msg),
            };
            (code, encode_error(json, error))
        }
    };
    (code, data)
}

pub fn encode_error(json: bool, error: impl Into<String>) -> Vec<u8> {
    if json {
        serde_json::to_string_pretty(&serde_json::json!({ "error": error.into() }))
            .unwrap_or_else(|_| r#"{"error": "failed to encode the error"}"#.to_string())
            .into_bytes()
    } else {
        encode_message_to_vec(&ProtoError::new(error.into()))
    }
}
