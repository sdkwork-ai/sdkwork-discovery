//! Zero-config discovery of the address this process should publish to the
//! cluster registry.
//!
//! Convention over configuration: an explicit private URL wins, then an
//! explicit advertise IP, then the first usable LAN interface (private IPv4
//! preferred), then the host name. Public edge domains are never published.

use std::net::{IpAddr, Ipv6Addr, UdpSocket};

/// Key for an authored private/LAN origin.
pub const ENV_PRIVATE_HTTP_URL: &str = "SDKWORK_PRIVATE_HTTP_URL";
/// Gateway-scoped private/LAN origin.
pub const ENV_GATEWAY_PRIVATE_HTTP_URL: &str = "SDKWORK_API_CLOUD_GATEWAY_PRIVATE_HTTP_URL";
/// Key for an explicit advertise IP.
pub const ENV_ADVERTISE_IP: &str = "SDKWORK_ADVERTISE_IP";
/// Gateway-scoped advertise IP.
pub const ENV_GATEWAY_ADVERTISE_IP: &str = "SDKWORK_API_CLOUD_GATEWAY_ADVERTISE_IP";

/// Absolute `http://host:port` origin this process publishes to the registry.
pub fn advertised_http_endpoint(port: u16) -> String {
    if let Some(url) = env_http_url(&[ENV_PRIVATE_HTTP_URL, ENV_GATEWAY_PRIVATE_HTTP_URL]) {
        return url;
    }
    if let Some(ip) = env_ip(&[ENV_ADVERTISE_IP, ENV_GATEWAY_ADVERTISE_IP])
        .or_else(detect_lan_ip)
        .filter(|ip| is_usable_lan(ip))
    {
        return http_origin(ip, port);
    }
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "127.0.0.1".to_owned());
    format!("http://{}:{}", host.trim(), port)
}

/// First usable LAN interface address, preferring private IPv4.
pub fn detect_lan_ip() -> Option<IpAddr> {
    let mut interfaces = if_addrs::get_if_addrs().ok()?;
    interfaces.sort_by_key(|interface| interface_score(interface.addr.ip()));
    interfaces
        .into_iter()
        .map(|interface| interface.addr.ip())
        .find(|ip| is_usable_lan(ip))
        .or_else(detect_default_route_ip)
}

/// Pure endpoint precedence, exposed for tests and diagnostics.
pub fn select_endpoint(
    port: u16,
    private_url: Option<&str>,
    advertise_ip: Option<IpAddr>,
    detected_ip: Option<IpAddr>,
    hostname: &str,
) -> String {
    if let Some(value) = private_url.map(str::trim).filter(|value| !value.is_empty()) {
        return value.trim_end_matches('/').to_owned();
    }
    if let Some(ip) = advertise_ip
        .into_iter()
        .chain(detected_ip)
        .find(|ip| is_usable_lan(ip))
    {
        return http_origin(ip, port);
    }
    format!("http://{}:{}", hostname.trim(), port)
}

fn env_http_url(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let value = std::env::var(key)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty())?;
        if value.starts_with("http://") || value.starts_with("https://") {
            Some(value)
        } else {
            tracing::warn!(key, "ignoring non-http(s) private base URL");
            None
        }
    })
}

fn env_ip(keys: &[&str]) -> Option<IpAddr> {
    keys.iter().find_map(|key| {
        let value = std::env::var(key)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())?;
        match value.parse::<IpAddr>() {
            Ok(ip) if is_usable_lan(&ip) => Some(ip),
            Ok(_) => {
                tracing::warn!(key, "ignoring loopback/unspecified advertise IP");
                None
            }
            Err(_) => {
                tracing::warn!(key, "ignoring invalid advertise IP");
                None
            }
        }
    })
}

fn http_origin(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(ip) => format!("http://{ip}:{port}"),
        IpAddr::V6(ip) => format!("http://[{ip}]:{port}"),
    }
}

fn is_usable_lan(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_unspecified() && !ip.is_broadcast(),
        IpAddr::V6(ip) => !ip.is_loopback() && !ip.is_unspecified(),
    }
}

fn interface_score(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(ip) if ip.is_private() => 0,
        IpAddr::V6(ip) if is_unique_local_v6(&ip) => 1,
        IpAddr::V4(ip) if ip.is_link_local() => 2,
        IpAddr::V4(_) => 3,
        IpAddr::V6(ip) if ip.is_unicast_link_local() => 4,
        IpAddr::V6(_) => 5,
    }
}

fn is_unique_local_v6(ip: &Ipv6Addr) -> bool {
    (ip.octets()[0] & 0xfe) == 0xfc
}

fn detect_default_route_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    is_usable_lan(&ip).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_url_wins_then_advertise_then_detected_then_hostname() {
        let ip: IpAddr = "10.1.2.3".parse().expect("ip");
        assert_eq!(
            select_endpoint(
                9000,
                Some("http://lan.example:8080/"),
                Some(ip),
                Some(ip),
                "host"
            ),
            "http://lan.example:8080"
        );
        assert_eq!(
            select_endpoint(9000, None, Some(ip), None, "host"),
            "http://10.1.2.3:9000"
        );
        assert_eq!(
            select_endpoint(9000, None, None, Some(ip), "host"),
            "http://10.1.2.3:9000"
        );
        assert_eq!(
            select_endpoint(9000, None, None, None, "host"),
            "http://host:9000"
        );
    }

    #[test]
    fn ipv6_endpoint_is_bracketed() {
        let ip: IpAddr = "fd00::1".parse().expect("ip");
        assert_eq!(
            select_endpoint(9000, None, Some(ip), None, "host"),
            "http://[fd00::1]:9000"
        );
    }
}
