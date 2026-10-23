//! Fetching URLs that users give us: only from the public internet, never
//! from addresses inside Toolforge (or anywhere private), redirects
//! included, and only up to a size limit.

use anyhow::{Context, Result, anyhow, bail};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REDIRECTS: usize = 5;

/// Resolves host names, keeping only public addresses.
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.filter(|a| is_public(a.ip())).collect();
            if addrs.is_empty() {
                return Err(format!("{host} has no public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

static CLIENT: LazyLock<reqwest::Result<reqwest::Client>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(crate::state::USER_AGENT)
        .dns_resolver(Arc::new(PublicOnly))
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let public = attempt.url().host_str().is_some_and(|h| h.parse::<IpAddr>().map_or(true, is_public));
            match (attempt.previous().len() >= MAX_REDIRECTS, public) {
                (false, true) => attempt.follow(),
                _ => attempt.stop(),
            }
        }))
        .build()
});

/// Downloads `url` into a temporary file of at most `max_bytes`.
pub async fn download(url: &str, max_bytes: u64) -> Result<(tempfile::NamedTempFile, u64)> {
    let parsed = reqwest::Url::parse(url).map_err(|_| anyhow!("not a URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("only http and https URLs can be fetched");
    }
    let host = parsed.host_str().ok_or_else(|| anyhow!("the URL has no host"))?;
    // IP addresses in the URL bypass the resolver.
    if host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok_and(|ip| !is_public(ip)) {
        bail!("that address is not on the public internet");
    }
    let client = CLIENT.as_ref().map_err(|e| anyhow!("HTTP client: {e}"))?;
    let mut response =
        client.get(parsed).send().await.context("could not download the source URL")?.error_for_status()?;
    if response.content_length().is_some_and(|l| l > max_bytes) {
        bail!("the file is larger than {} MB", max_bytes >> 20);
    }
    let file = tempfile::NamedTempFile::new().context("no temporary file")?;
    let mut writer = tokio::fs::File::from_std(file.reopen()?);
    let mut size = 0;
    while let Some(chunk) = response.chunk().await.context("the download broke off")? {
        size += chunk.len() as u64;
        if size > max_bytes {
            bail!("the file is larger than {} MB", max_bytes >> 20);
        }
        writer.write_all(&chunk).await?;
    }
    writer.flush().await?;
    if size == 0 {
        bail!("the source URL returned an empty file");
    }
    Ok((file, size))
}

/// Not loopback, private, link-local, shared, documentation or otherwise special.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || a >= 224)
        }
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => is_public(IpAddr::V4(v4)),
            None => {
                let first = ip.segments()[0];
                !(ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses() {
        for public in ["8.8.8.8", "208.80.154.224", "2620:0:861:ed1a::1"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        for private in [
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
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
    }

    #[tokio::test]
    async fn refuses_internal_urls() {
        for url in [
            "http://127.0.0.1/x",
            "http://[::1]/x",
            "ftp://example.org/x",
            "http://169.254.169.254/latest",
            "not a url",
        ] {
            assert!(download(url, 1000).await.is_err(), "{url}");
        }
    }
}
