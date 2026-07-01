//! Inter-process communication layer for out-of-process plugins.
//!
//! Built only with the `host` feature because it needs `acadrust`-typed
//! messages and the plugin runner binary.

#[cfg(feature = "host")]
pub mod client;
#[cfg(feature = "host")]
pub mod protocol;
#[cfg(feature = "host")]
pub mod server;
#[cfg(feature = "host")]
pub mod transport;

#[cfg(all(test, feature = "host"))]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use interprocess::local_socket::{
        traits::{Listener, Stream as StreamTrait},
        GenericNamespaced, ListenerOptions, Stream, ToNsName,
    };

    use crate::ipc::protocol::{HostRequest, HostResponse, HostToPlugin, PluginToHost};
    use crate::ipc::transport::{recv, send};

    fn unique_socket_name() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("ocs_plugin_test_{}_{}", std::process::id(), n)
    }

    fn connect_pair() -> (Stream, Stream) {
        let name = unique_socket_name();
        let name_ref = name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("valid namespaced name");
        let listener = ListenerOptions::new()
            .name(name_ref)
            .create_sync()
            .expect("create listener");
        let client = thread::spawn(move || {
            StreamTrait::connect(name.to_ns_name::<GenericNamespaced>().unwrap()).expect("connect")
        });
        let server = listener.accept().expect("accept");
        let client = client.join().expect("client thread");
        (server, client)
    }

    #[test]
    fn transport_round_trips_host_request() {
        let (mut a, mut b) = connect_pair();
        let req = HostRequest::Dispatch {
            cmd: "LINE".to_string(),
        };
        send(&mut a, &HostToPlugin::Request(req)).unwrap();
        let got = recv(&mut b).unwrap();
        match got {
            HostToPlugin::Request(HostRequest::Dispatch { cmd }) => assert_eq!(cmd, "LINE"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn transport_round_trips_plugin_request() {
        let (mut a, mut b) = connect_pair();
        let req = PluginToHost::Request(crate::ipc::protocol::PluginRequest::PushInfo(
            "hello".to_string(),
        ));
        send(&mut a, &req).unwrap();
        let got = recv(&mut b).unwrap();
        match got {
            PluginToHost::Request(crate::ipc::protocol::PluginRequest::PushInfo(msg)) => {
                assert_eq!(msg, "hello")
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn transport_rejects_oversized_message() {
        let (mut a, _b) = connect_pair();
        // A Vec<u8> larger than MAX_MESSAGE_SIZE should be rejected on send.
        let huge = vec![0u8; 65 * 1024 * 1024];
        let err = send(&mut a, &huge).unwrap_err();
        assert!(format!("{err}").contains("too large"));
    }

    #[test]
    fn protocol_host_response_serde_roundtrip() {
        let resp = HostResponse::Text("pick a point".to_string());
        let bytes = bincode::serialize(&resp).unwrap();
        let got: HostResponse = bincode::deserialize(&bytes).unwrap();
        match got {
            HostResponse::Text(s) => assert_eq!(s, "pick a point"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn protocol_plugin_to_host_serde_roundtrip() {
        let msg = PluginToHost::Response(HostResponse::Bool(true));
        let bytes = bincode::serialize(&msg).unwrap();
        let got: PluginToHost = bincode::deserialize(&bytes).unwrap();
        match got {
            PluginToHost::Response(HostResponse::Bool(b)) => assert!(b),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn protocol_panel_move_resize_serde_roundtrip() {
        use crate::ipc::protocol::PluginRequest;
        use crate::panel::PanelHandle;

        let handle = PanelHandle(7);
        let move_req = PluginRequest::MovePanel {
            handle,
            x: 12.5,
            y: 34.0,
        };
        let bytes = bincode::serialize(&PluginToHost::Request(move_req)).unwrap();
        let got: PluginToHost = bincode::deserialize(&bytes).unwrap();
        match got {
            PluginToHost::Request(PluginRequest::MovePanel { handle: h, x, y }) => {
                assert_eq!(h.0, 7);
                assert_eq!(x, 12.5);
                assert_eq!(y, 34.0);
            }
            other => panic!("unexpected: {other:?}"),
        }

        let resize_req = PluginRequest::ResizePanel {
            handle,
            width: 260.0,
            height: 400.0,
        };
        let bytes = bincode::serialize(&PluginToHost::Request(resize_req)).unwrap();
        let got: PluginToHost = bincode::deserialize(&bytes).unwrap();
        match got {
            PluginToHost::Request(PluginRequest::ResizePanel {
                handle: h,
                width,
                height,
            }) => {
                assert_eq!(h.0, 7);
                assert_eq!(width, 260.0);
                assert_eq!(height, 400.0);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn protocol_panel_event_serde_roundtrip() {
        use crate::panel::{DockZone, PanelEvent};

        let events = vec![
            PanelEvent::Moved { x: 10.0, y: 20.0 },
            PanelEvent::Resized {
                width: 100.0,
                height: 200.0,
            },
            PanelEvent::Focused,
            PanelEvent::Docked {
                zone: DockZone::Left,
            },
            PanelEvent::Undocked,
        ];
        for event in events {
            let bytes = bincode::serialize(&event).unwrap();
            let got: PanelEvent = bincode::deserialize(&bytes).unwrap();
            assert_eq!(got, event);
        }
    }
}
