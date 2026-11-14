//! `proxy.tlsProfile` settings: defaults, wire names and forward compatibility.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_core::settings::{
    ProxySettings, Settings, TlsProfile, TlsProfileOverride, TlsProfileSettings,
};

#[test]
fn defaults_to_native_without_overrides() {
    let s = ProxySettings::default();
    assert_eq!(s.tls_profile, TlsProfileSettings::default());
    assert_eq!(s.tls_profile.profile, TlsProfile::Native);
    assert!(s.tls_profile.overrides.is_empty());
    // A file without the setting loads with the default.
    let old: Settings = serde_json::from_str(r#"{"proxy":{"port":8123}}"#).unwrap();
    assert_eq!(old.proxy.port, 8123);
    assert_eq!(old.proxy.tls_profile.profile, TlsProfile::Native);
}

#[test]
fn wire_names_round_trip() {
    for p in TlsProfile::ALL {
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, format!("\"{}\"", p.as_str()));
        assert_eq!(serde_json::from_str::<TlsProfile>(&json).unwrap(), p);
        assert_eq!(TlsProfile::parse(p.as_str()), Some(p));
        assert_eq!(p.to_string(), p.as_str());
    }
    assert_eq!(TlsProfile::parse(" Chrome "), Some(TlsProfile::Chrome));
    assert_eq!(
        TlsProfile::parse("MIRROR-CLIENT"),
        Some(TlsProfile::MirrorClient)
    );
    assert_eq!(TlsProfile::parse("opera"), None);
    assert!(TlsProfile::Chrome.is_browser() && TlsProfile::Edge.is_browser());
    assert!(!TlsProfile::Native.is_browser() && !TlsProfile::MirrorClient.is_browser());
}

#[test]
fn settings_json_shape() {
    let settings = TlsProfileSettings {
        profile: TlsProfile::MirrorClient,
        overrides: vec![TlsProfileOverride {
            host: "*.shop.example".into(),
            profile: TlsProfile::Firefox,
        }],
    };
    let json = serde_json::to_value(&settings).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "profile": "mirror-client",
            "overrides": [{ "host": "*.shop.example", "profile": "firefox" }]
        })
    );
    let back: TlsProfileSettings = serde_json::from_value(json).unwrap();
    assert_eq!(back, settings);
    let proxy = serde_json::to_value(ProxySettings::default()).unwrap();
    assert_eq!(
        proxy["tlsProfile"],
        serde_json::json!({ "profile": "native", "overrides": [] })
    );
}

#[test]
fn unknown_profiles_from_newer_versions_do_not_break_the_settings_file() {
    let s: Settings = serde_json::from_str(
        r#"{"proxy":{"port":9000,"tlsProfile":{"profile":"chrome-android-200","overrides":[{"host":"a","profile":"brave"},{"host":"b","profile":"safari"}]}}}"#,
    )
    .unwrap();
    assert_eq!(s.proxy.port, 9000);
    assert_eq!(s.proxy.tls_profile.profile, TlsProfile::Native);
    assert_eq!(s.proxy.tls_profile.overrides[0].profile, TlsProfile::Native);
    assert_eq!(s.proxy.tls_profile.overrides[1].profile, TlsProfile::Safari);
}
