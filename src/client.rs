use std::sync::Arc;

use bevy::prelude::*;

pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let mut root_cert_store = rustls::RootCertStore::empty();
        let mut crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();
        crypto.alpn_protocols = vec![b"bevy".to_vec()];

        let config = quinn::ClientConfig::new(Arc::new(crypto));

        let mut endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(config);

        tokio::spawn(async move {
            let connection = endpoint
                .connect("127.0.0.1:8080".parse().unwrap(), "localhost")
                .unwrap()
                .await
                .unwrap();

            let (mut send, mut recv) = connection.open_bi().await.unwrap();
        });
    }
}
