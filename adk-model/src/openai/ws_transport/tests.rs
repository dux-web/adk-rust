use super::*;
use serde_json::json;
#[test]
fn preserves_endpoint_paths() {
    assert_eq!(
        derive_ws_url("https://chatgpt.com/backend-api/codex"),
        "wss://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(derive_ws_url("http://localhost:8080/v1/"), "ws://localhost:8080/v1/responses");
}
#[test]
fn extends_only_the_completed_baseline() {
    let request =
        json!({"model":"fixture", "store":false,"input":[{"role":"user","content":"first"}]});
    let output = json!({"id":"msg_1","status":"completed","role":"assistant","content":"answer"});
    let previous =
        Continuation { request: request.clone(), response: "resp_1".into(), output: vec![output] };
    let mut next = request;
    next["input"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":"answer"}),
        json!({"role":"user","content":"next"}),
    ]);
    let incremental = incremental_event(&next, &previous).unwrap();
    assert_eq!(incremental["previous_response_id"], "resp_1");
    assert_eq!(incremental["input"], json!([{"role":"user","content":"next"}]));
    next["model"] = json!("changed");
    assert!(incremental_event(&next, &previous).is_none());
    next["model"] = json!("fixture");
    next["input"][0]["content"] = json!("changed");
    assert!(incremental_event(&next, &previous).is_none());
}
#[test]
fn strips_transport_fields() {
    let event = create_event(
        &json!({"stream":true,"background":false,"previous_response_id":"caller","input":[]}),
    )
    .unwrap();
    assert_eq!(event, json!({"type":"response.create","previous_response_id":"caller","input":[]}));
}

#[test]
fn certificate_errors_are_definitive() {
    let invalid = || rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);
    for error in [
        tungstenite::Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, invalid())),
        tungstenite::Error::Tls(tungstenite::error::TlsError::Rustls(Box::new(invalid()))),
        tungstenite::Error::Tls(tungstenite::error::TlsError::InvalidDnsName),
    ] {
        let failure = classify(error);
        assert_eq!(failure.error.code, "model.openai_responses.ws_tls");
        assert!(!failure.error.is_retryable());
        assert_eq!(failure.refusal, WsRefusal::Definitive);
        assert_eq!(refusal(&failure.error), WsRefusal::Definitive);
    }
    for error in [
        tungstenite::Error::Io(std::io::ErrorKind::ConnectionReset.into()),
        tungstenite::Error::Tls(tungstenite::error::TlsError::Rustls(Box::new(
            rustls::Error::General("temporary failure".into()),
        ))),
    ] {
        let failure = classify(error);
        assert_eq!(failure.refusal, WsRefusal::Transient);
        assert_eq!(refusal(&failure.error), WsRefusal::Transient);
    }
}

#[tokio::test]
async fn transports_reject_untrusted_certificates() {
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use tokio::net::TcpListener;

    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![certificate.cert.der().clone()],
        PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("https://localhost:{}/v1", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            assert!(acceptor.accept(socket).await.is_err());
        }
    });
    let transport = WsTransport::lazy(&endpoint, "key", WsOptions::default())
        .with_retry_config(RetryConfig::default());
    let error = transport.preconnect(HeaderMap::new()).await.unwrap_err();
    assert_eq!(error.code, "model.openai_responses.ws_tls");
    assert!(!error.is_retryable());
    assert_eq!(
        transport.connection.lock().await.preconnect_failure.as_ref().unwrap().2,
        WsRefusal::Definitive
    );
    let error = reqwest_openai::Client::new().get(&endpoint).send().await.unwrap_err();
    assert!(error.is_connect());
    tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
}
