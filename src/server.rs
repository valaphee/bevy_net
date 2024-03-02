use std::sync::Arc;

use bevy::prelude::*;

pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let mut crypto = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(vec![rustls::Certificate(cert.serialize_der().unwrap())], rustls::PrivateKey(cert.serialize_private_key_der()))
            .unwrap();
        crypto.alpn_protocols = vec![b"bevy".to_vec()];

        let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        let transport_config = Arc::get_mut(&mut config.transport).unwrap();
        transport_config.max_concurrent_uni_streams(0_u8.into());

        let endpoint = quinn::Endpoint::server(config, "127.0.0.1:8080".parse().unwrap()).unwrap();

        tokio::spawn(async move {
            while let Some(conn) = endpoint.accept().await {
                let connection = conn.await.unwrap();

                let (mut send, mut recv) = connection.accept_bi().await.unwrap();
            }
        });
    }
}
