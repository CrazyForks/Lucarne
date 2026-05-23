//! Proxy env synthesis for child agent processes.
//!
//! Mirrors hyper-util/reqwest `system-proxy` by using the same OS proxy crates,
//! but exports env vars because hyper-util only answers per-URI proxy matches.

use std::collections::BTreeMap;

const PROXY_GROUPS: [(&str, &str); 4] = [
    ("HTTP_PROXY", "http_proxy"),
    ("HTTPS_PROXY", "https_proxy"),
    ("ALL_PROXY", "all_proxy"),
    ("NO_PROXY", "no_proxy"),
];

pub(crate) fn apply_missing_proxy_env(env: &mut Vec<(String, String)>) -> usize {
    let discovered = system_proxy_env();
    let overrides = missing_proxy_env_overrides(env, &discovered);
    let count = overrides.len();
    env.extend(overrides);
    count
}

fn missing_proxy_env_overrides(
    env: &[(String, String)],
    discovered: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (upper, lower) in PROXY_GROUPS {
        if has_env_key(env, upper) || has_env_key(env, lower) {
            continue;
        }
        let Some(value) = discovered.get(upper) else {
            continue;
        };
        out.push((upper.to_string(), value.clone()));
        out.push((lower.to_string(), value.clone()));
    }
    out
}

fn has_env_key(env: &[(String, String)], key: &str) -> bool {
    env.iter().any(|(existing, _)| existing == key)
}

#[cfg(target_os = "macos")]
fn system_proxy_env() -> BTreeMap<String, String> {
    macos::system_proxy_env()
}

#[cfg(windows)]
fn system_proxy_env() -> BTreeMap<String, String> {
    windows::system_proxy_env()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn system_proxy_env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn proxy_uri(scheme: &str, host: &str, port: Option<i32>) -> Option<String> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let has_scheme = host.contains("://");
    let mut uri = if has_scheme {
        host.to_string()
    } else {
        format!("{scheme}://{host}")
    };
    if let Some(port) = port {
        uri.push(':');
        uri.push_str(&port.to_string());
    }
    Some(uri)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{proxy_uri, BTreeMap};
    use system_configuration::core_foundation::array::CFArray;
    use system_configuration::core_foundation::base::{CFType, TCFType};
    use system_configuration::core_foundation::dictionary::CFDictionary;
    use system_configuration::core_foundation::number::CFNumber;
    use system_configuration::core_foundation::string::{CFString, CFStringRef};
    use system_configuration::dynamic_store::SCDynamicStoreBuilder;
    use system_configuration::sys::schema_definitions::{
        kSCPropNetProxiesExceptionsList, kSCPropNetProxiesHTTPEnable, kSCPropNetProxiesHTTPPort,
        kSCPropNetProxiesHTTPProxy, kSCPropNetProxiesHTTPSEnable, kSCPropNetProxiesHTTPSPort,
        kSCPropNetProxiesHTTPSProxy, kSCPropNetProxiesSOCKSEnable, kSCPropNetProxiesSOCKSPort,
        kSCPropNetProxiesSOCKSProxy,
    };

    pub(super) fn system_proxy_env() -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        let Some(store) = SCDynamicStoreBuilder::new("lucarne").build() else {
            return env;
        };
        let Some(proxies) = store.get_proxies() else {
            return env;
        };

        if let Some(proxy) = parse_proxy(
            &proxies,
            unsafe { kSCPropNetProxiesHTTPEnable },
            unsafe { kSCPropNetProxiesHTTPProxy },
            unsafe { kSCPropNetProxiesHTTPPort },
            "http",
        ) {
            env.insert("HTTP_PROXY".into(), proxy);
        }
        if let Some(proxy) = parse_proxy(
            &proxies,
            unsafe { kSCPropNetProxiesHTTPSEnable },
            unsafe { kSCPropNetProxiesHTTPSProxy },
            unsafe { kSCPropNetProxiesHTTPSPort },
            "http",
        ) {
            env.insert("HTTPS_PROXY".into(), proxy);
        }
        if let Some(proxy) = parse_proxy(
            &proxies,
            unsafe { kSCPropNetProxiesSOCKSEnable },
            unsafe { kSCPropNetProxiesSOCKSProxy },
            unsafe { kSCPropNetProxiesSOCKSPort },
            "socks5",
        ) {
            env.insert("ALL_PROXY".into(), proxy);
        }
        if let Some(no_proxy) = no_proxy(&proxies) {
            env.insert("NO_PROXY".into(), no_proxy);
        }
        env
    }

    fn parse_proxy(
        proxies: &CFDictionary<CFString, CFType>,
        enabled_key: CFStringRef,
        host_key: CFStringRef,
        port_key: CFStringRef,
        scheme: &str,
    ) -> Option<String> {
        let enabled = proxies
            .find(enabled_key)
            .and_then(|flag| flag.downcast::<CFNumber>())
            .and_then(|flag| flag.to_i32())
            .unwrap_or(0)
            == 1;
        if !enabled {
            return None;
        }
        let host = proxies
            .find(host_key)
            .and_then(|host| host.downcast::<CFString>())
            .map(|host| host.to_string())?;
        let port = proxies
            .find(port_key)
            .and_then(|port| port.downcast::<CFNumber>())
            .and_then(|port| port.to_i32());
        proxy_uri(scheme, &host, port)
    }

    fn no_proxy(proxies: &CFDictionary<CFString, CFType>) -> Option<String> {
        let array = proxies
            .find(unsafe { kSCPropNetProxiesExceptionsList })
            .and_then(|value| value.downcast::<CFArray>())?;
        let values = array
            .get_all_values()
            .into_iter()
            .filter_map(|value| {
                let value = unsafe { CFType::wrap_under_get_rule(value.cast()) };
                value.downcast::<CFString>().map(|value| value.to_string())
            })
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>();
        if values.is_empty() {
            None
        } else {
            Some(values.join(","))
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::{windows_proxy_server_env, BTreeMap};

    const INTERNET_SETTINGS: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

    pub(super) fn system_proxy_env() -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        let Ok(settings) = windows_registry::CURRENT_USER.open(INTERNET_SETTINGS) else {
            return env;
        };
        if settings.get_u32("ProxyEnable").unwrap_or(0) == 0 {
            return env;
        }
        if let Ok(server) = settings.get_string("ProxyServer") {
            env.extend(windows_proxy_server_env(&server));
        }
        if let Ok(override_list) = settings.get_string("ProxyOverride") {
            if let Some(no_proxy) = windows_no_proxy(&override_list) {
                env.insert("NO_PROXY".into(), no_proxy);
            }
        }
        env
    }

    fn windows_no_proxy(raw: &str) -> Option<String> {
        let values = raw
            .split(';')
            .filter_map(|entry| {
                let entry = entry.trim();
                if entry.is_empty() || entry.eq_ignore_ascii_case("<local>") {
                    return None;
                }
                Some(entry.strip_prefix("*.").unwrap_or(entry).to_string())
            })
            .collect::<Vec<_>>();
        if values.is_empty() {
            None
        } else {
            Some(values.join(","))
        }
    }
}

#[cfg(any(windows, test))]
fn windows_proxy_server_env(raw: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if raw.contains('=') {
        for part in raw.split(';') {
            let Some((kind, target)) = part.split_once('=') else {
                continue;
            };
            let kind = kind.trim().to_ascii_lowercase();
            let target = target.trim();
            match kind.as_str() {
                "http" => {
                    if let Some(proxy) = proxy_uri("http", target, None) {
                        env.insert("HTTP_PROXY".into(), proxy);
                    }
                }
                "https" => {
                    if let Some(proxy) = proxy_uri("http", target, None) {
                        env.insert("HTTPS_PROXY".into(), proxy);
                    }
                }
                "socks" | "socks5" => {
                    if let Some(proxy) = proxy_uri("socks5", target, None) {
                        env.insert("ALL_PROXY".into(), proxy);
                    }
                }
                _ => {}
            }
        }
        return env;
    }

    if let Some(proxy) = proxy_uri("http", raw, None) {
        env.insert("HTTP_PROXY".into(), proxy.clone());
        env.insert("HTTPS_PROXY".into(), proxy);
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_proxy_env_overrides_fills_upper_and_lower_without_overwriting_group() {
        let env = vec![("HTTPS_PROXY".to_string(), "http://custom:9443".to_string())];
        let discovered = BTreeMap::from([
            (
                "HTTP_PROXY".to_string(),
                "http://127.0.0.1:6152".to_string(),
            ),
            (
                "HTTPS_PROXY".to_string(),
                "http://127.0.0.1:6152".to_string(),
            ),
            (
                "ALL_PROXY".to_string(),
                "socks5://127.0.0.1:6153".to_string(),
            ),
            ("NO_PROXY".to_string(), "localhost".to_string()),
        ]);

        let overrides = missing_proxy_env_overrides(&env, &discovered);

        assert!(overrides.contains(&("HTTP_PROXY".into(), "http://127.0.0.1:6152".into())));
        assert!(overrides.contains(&("http_proxy".into(), "http://127.0.0.1:6152".into())));
        assert!(!overrides.iter().any(|(key, _)| key == "HTTPS_PROXY"));
        assert!(!overrides.iter().any(|(key, _)| key == "https_proxy"));
        assert!(overrides.contains(&("ALL_PROXY".into(), "socks5://127.0.0.1:6153".into())));
        assert!(overrides.contains(&("NO_PROXY".into(), "localhost".into())));
    }

    #[test]
    fn proxy_uri_adds_scheme_and_optional_port() {
        assert_eq!(
            proxy_uri("http", "127.0.0.1", Some(6152)).as_deref(),
            Some("http://127.0.0.1:6152")
        );
        assert_eq!(
            proxy_uri("http", "http://127.0.0.1:6152", None).as_deref(),
            Some("http://127.0.0.1:6152")
        );
    }

    #[test]
    fn windows_proxy_server_parses_simple_and_per_scheme_values() {
        let simple = windows_proxy_server_env("127.0.0.1:6152");
        assert_eq!(
            simple.get("HTTP_PROXY").map(String::as_str),
            Some("http://127.0.0.1:6152")
        );
        assert_eq!(
            simple.get("HTTPS_PROXY").map(String::as_str),
            Some("http://127.0.0.1:6152")
        );

        let per_scheme = windows_proxy_server_env(
            "http=127.0.0.1:6152;https=secure.proxy:8443;socks=127.0.0.1:6153",
        );
        assert_eq!(
            per_scheme.get("HTTP_PROXY").map(String::as_str),
            Some("http://127.0.0.1:6152")
        );
        assert_eq!(
            per_scheme.get("HTTPS_PROXY").map(String::as_str),
            Some("http://secure.proxy:8443")
        );
        assert_eq!(
            per_scheme.get("ALL_PROXY").map(String::as_str),
            Some("socks5://127.0.0.1:6153")
        );
    }
}
