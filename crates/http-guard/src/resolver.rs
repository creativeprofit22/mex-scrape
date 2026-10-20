//! DNS resolver that only answers for allowlisted hosts and drops
//! loopback/private/link-local addresses (DNS-rebinding guard). In
//! private-service mode (ADR 0013) loopback and private answers are kept
//! but link-local/metadata, multicast and unspecified ones are still dropped.

use std::net::SocketAddr;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::ip::{is_forbidden_service_ip, is_non_public_ip};
use crate::policy::HostAllowlist;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Resolver used by every non-test client.
#[derive(Debug)]
pub(crate) struct PublicOnlyResolver {
    allowlist: Arc<HostAllowlist>,
    private_service: bool,
}

impl PublicOnlyResolver {
    pub(crate) fn new(allowlist: Arc<HostAllowlist>) -> Self {
        Self {
            allowlist,
            private_service: false,
        }
    }

    /// Resolver for [`crate::TransportPolicy::private_service`].
    pub(crate) fn private_service(allowlist: Arc<HostAllowlist>) -> Self {
        Self {
            allowlist,
            private_service: true,
        }
    }
}

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let allowed = self.allowlist.contains(&host);
        let private_service = self.private_service;
        Box::pin(async move {
            if !allowed {
                return Err::<Addrs, BoxError>(
                    format!("host {host} is not on this client's allowlist").into(),
                );
            }
            let public: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| {
                    if private_service {
                        !is_forbidden_service_ip(address.ip())
                    } else {
                        !is_non_public_ip(address.ip())
                    }
                })
                .collect();
            if public.is_empty() {
                let why = if private_service {
                    "destination host resolves only to link-local, multicast or unspecified addresses"
                } else {
                    "destination host resolves only to local or private network addresses"
                };
                return Err(why.into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PublicOnlyResolver;
    use crate::policy::HostAllowlist;
    use reqwest::dns::Resolve as _;
    use std::sync::Arc;

    fn resolver(hosts: &[&str]) -> PublicOnlyResolver {
        PublicOnlyResolver::new(Arc::new(HostAllowlist::new(hosts).unwrap()))
    }

    #[tokio::test]
    async fn public_only_resolver_drops_local_answers() {
        let result = resolver(&["localhost"])
            .resolve("localhost".parse().unwrap())
            .await;
        let error = result
            .err()
            .expect("localhost must not resolve for public destinations");
        assert!(error.to_string().contains("private network"), "{error}");
    }

    #[tokio::test]
    async fn private_service_resolver_keeps_loopback_answers() {
        let resolver = PublicOnlyResolver::private_service(Arc::new(
            HostAllowlist::new(["localhost"]).unwrap(),
        ));
        let addrs: Vec<_> = resolver
            .resolve("localhost".parse().unwrap())
            .await
            .expect("localhost resolves for a private service")
            .collect();
        assert!(!addrs.is_empty());
        assert!(addrs.iter().all(|a| a.ip().is_loopback()), "{addrs:?}");
        let off_list = resolver.resolve("example.com".parse().unwrap()).await;
        assert!(off_list.is_err());
    }

    #[tokio::test]
    async fn resolver_refuses_hosts_off_the_allowlist() {
        let result = resolver(&["api.brightdata.com"])
            .resolve("localhost".parse().unwrap())
            .await;
        let error = result.err().expect("unlisted host must not resolve");
        assert!(error.to_string().contains("allowlist"), "{error}");
    }
}
