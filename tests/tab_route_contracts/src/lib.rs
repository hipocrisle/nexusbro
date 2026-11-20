// Import the real contract module; this harness has no CEF/Tauri dependency.
#[path = "../../../desktop/src-tauri/src/tab_routes.rs"]
pub mod tab_routes;
#[path = "../../../desktop/src-tauri/src/memory_guard.rs"]
pub mod memory_guard;

#[cfg(test)]
mod tests {
    use super::tab_routes::*;
    use serde::{Deserialize, Serialize};
    use serde_json::{json, Value};

    #[derive(Debug, Deserialize, Serialize)]
    struct Tab {
        id: u64,
        title: String,
        #[serde(default)]
        route: RouteChoice,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Session {
        tabs: Vec<Tab>,
        active: u64,
    }

    fn choice(value: Value) -> RouteChoice {
        serde_json::from_value(value)
            .expect("a malformed route must become blocked, not reject its row")
    }

    fn connection(id: &str) -> RouteChoice {
        RouteChoice::Connection {
            id: ConnectionId::parse(id).unwrap(),
        }
    }

    #[test]
    fn only_a_missing_containing_field_uses_legacy_inheritance() {
        let old: Tab = serde_json::from_value(json!({"id":7,"title":"legacy"})).unwrap();
        assert_eq!(old.route, RouteChoice::Inherit);
        for supplied in [
            Value::Null,
            json!(false),
            json!(0),
            json!("inherit"),
            json!([]),
            json!({}),
        ] {
            let tab: Tab =
                serde_json::from_value(json!({"id":7,"title":"legacy","route":supplied})).unwrap();
            assert_eq!(
                tab.route,
                RouteChoice::Blocked {
                    reason: RouteBlockReason::MalformedIntent
                }
            );
        }
    }

    #[test]
    fn exact_known_wire_shapes_round_trip_without_network_configuration() {
        for value in [
            json!({"kind":"inherit"}),
            json!({"kind":"direct"}),
            json!({"kind":"connection","id":"vyh123"}),
            json!({"kind":"connection","id":"A123_legacy.id-1"}),
            json!({"kind":"connection","id":"550e8400-e29b-41d4-a716-446655440000"}),
        ] {
            assert_eq!(serde_json::to_value(choice(value.clone())).unwrap(), value);
        }
    }

    #[test]
    fn unknown_future_intent_and_invalid_rows_do_not_destroy_the_session() {
        let session: Session = serde_json::from_value(json!({"active":3,"tabs":[
            {"id":1,"title":"old"},
            {"id":2,"title":"future","route":{"kind":"future_transport","endpoint":"socks5://user:secret@host:1234"}},
            {"id":3,"title":"direct","route":{"kind":"direct"}},
            {"id":4,"title":"malformed","route":{"kind":"connection","id":42}}
        ]})).unwrap();
        assert_eq!(session.active, 3);
        assert_eq!(
            session.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(session.tabs[0].route, RouteChoice::Inherit);
        assert_eq!(
            session.tabs[1].route,
            RouteChoice::Blocked {
                reason: RouteBlockReason::UnknownIntent
            }
        );
        assert_eq!(session.tabs[2].route, RouteChoice::Direct);
        assert_eq!(
            session.tabs[3].route,
            RouteChoice::Blocked {
                reason: RouteBlockReason::MalformedIntent
            }
        );
        assert_eq!(session.tabs[1].title, "future");
        assert!(!serde_json::to_string(&session).unwrap().contains("secret"));
    }

    #[test]
    fn extra_fields_and_missing_or_wrong_keys_are_not_silently_accepted() {
        for value in [
            json!({"kind":"inherit","id":"vyh1"}),
            json!({"kind":"direct","proxy":"https://secret:secret@host"}),
            json!({"kind":"connection"}),
            json!({"kind":"connection","id":"vyh1","port":1234}),
            json!({"kind":"connection","other":"vyh1"}),
            json!({"kind":1}),
            json!({"kind":"blocked","reason":"secret"}),
        ] {
            assert_eq!(
                choice(value),
                RouteChoice::Blocked {
                    reason: RouteBlockReason::MalformedIntent
                }
            );
        }
    }

    #[test]
    fn duplicate_route_keys_block_instead_of_last_key_winning() {
        for value in [
            r#"{"id":1,"title":"keep row","route":{"kind":"connection","id":"vyh1","kind":"direct"}}"#,
            r#"{"id":1,"title":"keep row","route":{"kind":"connection","id":"vyh1","id":"vyh2"}}"#,
        ] {
            let tab: Tab = serde_json::from_str(value).unwrap();
            assert_eq!(tab.id, 1);
            assert_eq!(tab.title, "keep row");
            assert_eq!(
                tab.route,
                RouteChoice::Blocked {
                    reason: RouteBlockReason::MalformedIntent
                }
            );
        }
    }

    #[test]
    fn opaque_ids_are_bounded_and_never_trimmed_or_treated_as_paths_or_urls() {
        let maximum = "A".repeat(MAX_CONNECTION_ID_BYTES);
        assert_eq!(ConnectionId::parse(&maximum).unwrap().as_str(), maximum);
        for id in [
            "",
            " ",
            " vyh1",
            "vyh1 ",
            "../vyh1",
            ".hidden",
            "a/b",
            "a\\b",
            "C:\\private",
            "https://host/path",
            "socks5://user:secret@host:1234",
            "host:1234",
            "a\0b",
            "a\nb",
            "аbc",
            "a?token=secret",
        ] {
            assert_eq!(
                ConnectionId::parse(id),
                Err(RouteBlockReason::InvalidConnectionId)
            );
            assert_eq!(
                choice(json!({"kind":"connection","id":id})),
                RouteChoice::Blocked {
                    reason: RouteBlockReason::InvalidConnectionId
                }
            );
        }
        assert_eq!(
            ConnectionId::parse(&"A".repeat(MAX_CONNECTION_ID_BYTES + 1)),
            Err(RouteBlockReason::InvalidConnectionId)
        );
    }

    #[test]
    fn blocked_state_survives_reserialization_without_restoring_raw_input() {
        for value in [
            json!({"kind":"future_secret","url":"https://user:secret@host"}),
            json!({"kind":"connection","id":"https://user:secret@host"}),
            json!({"kind":"direct","secret":"credentials"}),
        ] {
            let blocked = choice(value);
            assert!(matches!(blocked, RouteChoice::Blocked { .. }));
            let persisted = serde_json::to_value(&blocked).unwrap();
            assert_eq!(persisted.as_object().unwrap().len(), 2);
            assert_eq!(persisted["kind"], "blocked");
            assert_eq!(choice(persisted), blocked);
            assert!(!format!("{blocked:?}").contains("secret"));
        }
    }

    #[test]
    fn diagnostics_never_repeat_input_values_even_for_valid_opaque_ids() {
        let valid = connection("SensitiveOpaqueIdentifier123");
        let debug = format!("{valid:?}");
        assert!(!debug.contains("SensitiveOpaqueIdentifier123"));
        let error = valid.resolve([]).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("SensitiveOpaqueIdentifier123"));
        for value in [
            json!({"kind":"https://secret@host"}),
            json!({"kind":"connection","id":"socks5://user:secret@host"}),
        ] {
            let blocked = choice(value);
            let error = blocked.resolve([]).unwrap_err();
            assert!(!format!("{blocked:?} {error:?} {error}").contains("secret"));
        }
    }

    #[test]
    fn resolution_uses_exact_id_membership_not_list_position_or_name() {
        let route = connection("Connection_B");
        for inventory in [
            ["Connection_A", "Connection_B", "Connection_C"],
            ["Connection_C", "Connection_A", "Connection_B"],
        ] {
            let ResolvedRoute::Connection { id } = route.resolve(inventory).unwrap() else {
                panic!("must retain explicit connection");
            };
            assert_eq!(id.as_str(), "Connection_B");
        }
        assert_eq!(
            route.resolve(["connection_b"]),
            Err(RouteResolveError::MissingConnection)
        );
    }

    #[test]
    fn missing_removed_and_duplicate_selected_ids_are_explicit_errors_without_fallback() {
        let route = connection("vyh1");
        assert_eq!(route.resolve([]), Err(RouteResolveError::MissingConnection));
        assert_eq!(
            route.resolve(["replacement"]),
            Err(RouteResolveError::MissingConnection)
        );
        assert_eq!(
            route.resolve(["vyh1", "other", "vyh1"]),
            Err(RouteResolveError::AmbiguousConnection)
        );
        assert!(matches!(
            route.resolve(["unrelated", "unrelated", "vyh1"]),
            Ok(ResolvedRoute::Connection { .. })
        ));
    }

    #[test]
    fn inherit_direct_and_blocked_never_consult_or_modify_the_super_policy() {
        fn unused_inventory() -> impl Iterator<Item = &'static str> {
            std::iter::from_fn(|| panic!("inventory must not be consulted for this route"))
        }
        assert_eq!(
            RouteChoice::Inherit.resolve(unused_inventory()),
            Ok(ResolvedRoute::Inherit)
        );
        assert_eq!(
            RouteChoice::Direct.resolve(unused_inventory()),
            Ok(ResolvedRoute::Direct)
        );
        assert_eq!(
            RouteChoice::Blocked {
                reason: RouteBlockReason::UnknownIntent
            }
            .resolve(unused_inventory()),
            Err(RouteResolveError::Blocked(RouteBlockReason::UnknownIntent))
        );
    }
}

#[cfg(test)]
mod transport_validation {
    use super::tab_routes::NativeRoute;
    #[test]
    fn only_local_http_transport_and_fixed_opaque_keys_are_allowed() {
        let key = "a".repeat(64);
        assert!(NativeRoute { allow_non_proxied_udp: false, key: key.clone(), proxy: "http://127.0.0.1:12345".into() }.validate());
        for proxy in ["direct://", "http://127.0.0.1:0", "http://localhost:80", "http://127.0.0.1:65536", "http://127.0.0.1:80;direct://", "http://user:password@127.0.0.1:80", "http://127.0.0.1:+80", "http://127.0.0.1:80/"] {
            assert!(!NativeRoute { allow_non_proxied_udp: false, key: key.clone(), proxy: proxy.into() }.validate());
        }
        for bad in ["../Default".to_string(), "g".repeat(64), "A".repeat(64), "a".repeat(63)] {
            assert!(!NativeRoute { allow_non_proxied_udp: false, key: bad, proxy: "http://127.0.0.1:80".into() }.validate());
        }
    }
}
