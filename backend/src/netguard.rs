//! Where organisations' connections may connect (server-side request forgery protection).
//!
//! Connections configured by an organisation make the server open network connections, so they
//! must not reach the platform's internals: loopback, private and link-local networks, cloud
//! metadata, nor the platform's own databases. A host name is resolved and every address checked;
//! callers connect to the checked address so a second DNS answer can't redirect them.
//!
//! Private networks can be allowed for development (MAGNUM_ALLOW_PRIVATE_CONNECTIONS, on unless
//! MAGNUM_ENV=production) or per host (MAGNUM_CONNECTION_ALLOWED_HOSTS). The platform's own
//! database endpoints are always refused.

use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, Default)]
pub struct NetGuard {
    pub allow_private: bool,
    /// host names (or IPs) allowed even on private addresses, lowercase
    pub allowed_hosts: Vec<String>,
    /// the platform's own databases, as `host:port`: never reachable from a connection
    pub internal: Vec<String>,
}

/// Loopback, private, link-local (cloud metadata), shared, multicast and unspecified addresses.
pub fn is_internal_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1])) // carrier-grade NAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link-local
        }
    }
}

const REFUSED: &str = "connections to internal or private network addresses aren't allowed here";

impl NetGuard {
    /// Resolve `host:port` and check every address; returns the one to connect to.
    pub async fn check(&self, host: &str, port: u16) -> Result<SocketAddr, String> {
        let host = host.trim().trim_start_matches('[').trim_end_matches(']').to_lowercase();
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| format!("can't resolve {host:?}"))?
            .collect();
        let first = *addrs.first().ok_or_else(|| format!("can't resolve {host:?}"))?;
        for internal in &self.internal {
            let theirs: Vec<SocketAddr> = match tokio::net::lookup_host(internal.as_str()).await {
                Ok(a) => a.collect(),
                Err(_) => continue,
            };
            if addrs.iter().any(|a| theirs.iter().any(|t| t.port() == a.port() && same_ip(t.ip(), a.ip()))) {
                return Err(REFUSED.into());
            }
        }
        let allowed = self.allow_private || self.allowed_hosts.contains(&host);
        if !allowed && addrs.iter().any(|a| is_internal_ip(a.ip())) {
            return Err(REFUSED.into());
        }
        Ok(first)
    }

    /// The host and port of a URL-ish location (`https://host[:port]/...`, `host[:port]`).
    pub async fn check_url(&self, location: &str, default_port: u16) -> Result<(), String> {
        let rest = location.split_once("://").map_or(location, |(_, r)| r);
        let authority = rest.split('/').next().unwrap_or_default();
        let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a); // no credentials in URLs
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || authority.starts_with('[') => match p.parse() {
                Ok(p) => (h, p),
                Err(_) => (authority, default_port),
            },
            _ => (authority, default_port),
        };
        self.check(host, port).await.map(|_| ())
    }
}

fn same_ip(a: IpAddr, b: IpAddr) -> bool {
    // "localhost" and "127.0.0.1" are the same machine; so are all loopback addresses
    a == b || (a.is_loopback() && b.is_loopback()) || (a.is_unspecified() || b.is_unspecified())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(is_internal_ip(ip.parse().unwrap()), "{ip} is internal");
        }
        for ip in ["8.8.8.8", "35.190.247.13", "2001:4860:4860::8888"] {
            assert!(!is_internal_ip(ip.parse().unwrap()), "{ip} is public");
        }
    }

    #[tokio::test]
    async fn private_addresses_are_refused_unless_allowed() {
        let strict = NetGuard::default();
        for (host, port) in [("127.0.0.1", 5432), ("localhost", 5432), ("169.254.169.254", 80), ("[::1]", 5432)] {
            assert!(strict.check(host, port).await.is_err(), "{host}");
        }
        assert!(strict.check("10.0.0.5", 5432).await.is_err());
        let allowlisted = NetGuard { allowed_hosts: vec!["10.0.0.5".into()], ..NetGuard::default() };
        assert_eq!(allowlisted.check("10.0.0.5", 5432).await.unwrap().to_string(), "10.0.0.5:5432");
        let dev = NetGuard { allow_private: true, ..NetGuard::default() };
        assert!(dev.check("127.0.0.1", 5432).await.is_ok());
    }

    #[tokio::test]
    async fn the_platforms_own_databases_are_always_refused() {
        let dev = NetGuard { allow_private: true, internal: vec!["127.0.0.1:55432".into()], ..NetGuard::default() };
        assert!(dev.check("localhost", 55432).await.is_err(), "same machine, other spelling");
        assert!(dev.check("127.0.0.1", 55440).await.is_ok(), "another port is another service");
    }

    #[tokio::test]
    async fn urls() {
        let strict = NetGuard::default();
        for url in [
            "https://127.0.0.1/x.parquet",
            "https://user:pw@localhost:8443/x.csv",
            "169.254.169.254:80",
            "http://[::1]/x",
        ] {
            assert!(strict.check_url(url, 443).await.is_err(), "{url}");
        }
    }
}
