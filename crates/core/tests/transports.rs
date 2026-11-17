//! End-to-end transport tests over loopback.

use std::time::{Duration, Instant};

use termbridge_core::{Direction, SessionManager, TcpClientConfig, TcpServerConfig, UdpConfig};

fn port_from_peer(peer: &str) -> u16 {
    peer.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("peer address should end in a port")
}

async fn wait_for_bytes(
    manager: &SessionManager,
    id: &str,
    needle: &[u8],
    timeout: Duration,
) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut since = 0u64;
    let mut collected = Vec::new();
    while Instant::now() < deadline {
        let batch = manager.read_since(id, since, 64 * 1024).unwrap();
        since = batch.next_seq;
        for chunk in batch.chunks {
            if chunk.dir == Direction::Rx {
                collected.extend_from_slice(&chunk.data);
            }
        }
        if collected.windows(needle.len()).any(|w| w == needle) {
            return collected;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    collected
}

#[tokio::test]
async fn tcp_echo_roundtrip() {
    let manager = SessionManager::new();
    let server = manager
        .open_tcp_server(TcpServerConfig {
            bind: "127.0.0.1".into(),
            port: 0,
            echo: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let port = port_from_peer(server.peer.as_deref().unwrap());

    let client = manager
        .open_tcp_client(TcpClientConfig {
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        })
        .await
        .unwrap();
    manager
        .wait_connected(&client.id, Duration::from_secs(3))
        .await
        .unwrap();

    // Give the server a moment to register the client.
    tokio::time::sleep(Duration::from_millis(100)).await;
    manager.write(&client.id, b"hello tcp").unwrap();

    let received = wait_for_bytes(&manager, &client.id, b"hello tcp", Duration::from_secs(3)).await;
    assert_eq!(received, b"hello tcp");

    let server_info = manager.info(&server.id).unwrap();
    assert_eq!(server_info.clients.len(), 1);
    assert!(server_info.rx_bytes >= 9);

    manager.close_all();
}

#[tokio::test]
async fn udp_echo_roundtrip() {
    let manager = SessionManager::new();
    let peer = manager
        .open_udp(UdpConfig {
            bind: "127.0.0.1".into(),
            local_port: 0,
            echo: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let port = port_from_peer(peer.peer.as_deref().unwrap());

    let client = manager
        .open_udp(UdpConfig {
            bind: "127.0.0.1".into(),
            local_port: 0,
            remote_host: Some("127.0.0.1".into()),
            remote_port: Some(port),
            ..Default::default()
        })
        .await
        .unwrap();
    manager
        .wait_connected(&client.id, Duration::from_secs(2))
        .await
        .unwrap();

    manager.write(&client.id, b"ping").unwrap();
    let received = wait_for_bytes(&manager, &client.id, b"ping", Duration::from_secs(3)).await;
    assert_eq!(received, b"ping");

    let peer_info = manager.info(&peer.id).unwrap();
    assert_eq!(peer_info.rx_bytes, 4);
    manager.close_all();
}

#[tokio::test]
async fn sessions_report_counters_and_close() {
    let manager = SessionManager::new();
    let server = manager
        .open_tcp_server(TcpServerConfig {
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(manager.list().len(), 1);
    assert!(manager.info(&server.id).unwrap().connected);

    manager.close(&server.id).unwrap();
    assert!(manager.list().is_empty());
    assert!(manager.info(&server.id).is_err());
}

#[tokio::test]
async fn missing_serial_port_is_reported() {
    let manager = SessionManager::new();
    let result = manager
        .open_serial(termbridge_core::SerialConfig {
            port: "termbridge-no-such-port".into(),
            ..Default::default()
        })
        .await;
    assert!(result.is_err());
    assert!(manager.list().is_empty());
}

#[tokio::test]
async fn control_lines_are_validated_per_transport() {
    let manager = SessionManager::new();
    let server = manager
        .open_tcp_server(TcpServerConfig {
            bind: "127.0.0.1".into(),
            port: 0,
            echo: false,
            ..Default::default()
        })
        .await
        .unwrap();

    // Echo is valid on a TCP server.
    manager
        .set_control(&server.id, termbridge_core::ControlOp::Echo(true))
        .unwrap();
    // RTS is not.
    assert!(manager
        .set_control(&server.id, termbridge_core::ControlOp::Rts(true))
        .is_err());
    // Unknown sessions are rejected.
    assert!(manager.write("nope", b"x").is_err());
    manager.close_all();
}

#[tokio::test]
async fn auto_responder_and_csv_recording() {
    use termbridge_core::logging::LogFormat;
    use termbridge_core::responder::RuleDef;

    let manager = SessionManager::new();
    let server = manager
        .open_tcp_server(TcpServerConfig {
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        })
        .await
        .unwrap();
    let port = port_from_peer(server.peer.as_deref().unwrap());
    // The server behaves like a modem: AT -> OK.
    manager
        .set_auto_responses(
            &server.id,
            &[RuleDef {
                pattern: "AT\r".into(),
                reply: "OK\r\n".into(),
                ..Default::default()
            }],
        )
        .unwrap();
    let dir = std::env::temp_dir().join(format!("termbridge-rec-{}", std::process::id()));
    let path = dir.join("server.csv");
    manager
        .set_logging_with(&server.id, Some(path.clone()), false, LogFormat::Csv)
        .unwrap();

    let client = manager
        .open_tcp_client(TcpClientConfig {
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        })
        .await
        .unwrap();
    manager
        .wait_connected(&client.id, Duration::from_secs(3))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Split the command across two writes to exercise cross-chunk matching.
    manager.write(&client.id, b"A").unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    manager.write(&client.id, b"T\r").unwrap();

    let received = wait_for_bytes(&manager, &client.id, b"OK\r\n", Duration::from_secs(3)).await;
    assert!(received.ends_with(b"OK\r\n"), "got {received:?}");
    let status = manager.auto_responses(&server.id).unwrap();
    assert_eq!(status[0].hits, 1);
    let info = manager.info(&server.id).unwrap();
    assert_eq!(info.auto_responses, 1);
    assert!(info.recording.is_some());

    manager.set_logging(&server.id, None, false).unwrap();
    let csv = std::fs::read_to_string(&path).unwrap();
    assert!(csv.starts_with("timestamp,direction,length,hex,text"));
    assert!(csv.contains(",TX,4,4F 4B 0D 0A,"), "csv was {csv}");
    manager.close_all();
    let _ = std::fs::remove_dir_all(dir);
}
