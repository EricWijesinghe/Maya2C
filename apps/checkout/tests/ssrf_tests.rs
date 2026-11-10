//! SSRF threats: a shop-supplied webhook URL must never reach the service's own
//! network. No DNS: resolution is passed in, so these run offline.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::IpAddr;

use maya_checkout::ssrf::{MAX_URL_BYTES, UrlError, is_public_ip, parse_https, pin};

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn public_https_url_is_accepted() {
    assert!(parse_https("https://shop.example.com/hooks/maya").is_ok());
    assert!(parse_https("https://93.184.216.34/hook").is_ok());
}

#[test]
fn only_https_is_accepted() {
    assert_eq!(parse_https("http://shop.example.com/hook"), Err(UrlError::Scheme));
    assert_eq!(parse_https("ftp://shop.example.com/hook"), Err(UrlError::Scheme));
    assert_eq!(parse_https("file:///etc/passwd"), Err(UrlError::Scheme));
}

#[test]
fn credentials_in_the_url_are_refused() {
    assert_eq!(
        parse_https("https://user:pass@shop.example.com/hook"),
        Err(UrlError::Credentials)
    );
    assert_eq!(parse_https("https://user@shop.example.com/hook"), Err(UrlError::Credentials));
}

#[test]
fn local_and_internal_names_are_refused() {
    for host in [
        "https://localhost/hook",
        "https://localhost./hook",
        "https://api.localhost/hook",
        "https://metadata.internal/hook",
        "https://printer.local/hook",
        "https://router.lan/hook",
        "https://intranet/hook",
    ] {
        assert_eq!(parse_https(host), Err(UrlError::HostName), "accepted {host}");
    }
}

#[test]
fn private_ip_literals_are_refused_before_any_lookup() {
    for literal in [
        "https://127.0.0.1/hook",
        "https://10.1.2.3/hook",
        "https://172.16.0.9/hook",
        "https://192.168.1.1/hook",
        "https://169.254.169.254/latest/meta-data",
        "https://100.64.0.1/hook",
        "https://0.0.0.0/hook",
        "https://[::1]/hook",
        "https://[fc00::1]/hook",
        "https://[fe80::1]/hook",
        "https://[::ffff:127.0.0.1]/hook",
    ] {
        let url = parse_https(literal).expect(literal);
        assert_eq!(pin(url, &[]).map(|_| ()), Err(UrlError::NotPublic), "accepted {literal}");
    }
}

#[test]
fn overlong_urls_are_refused() {
    let long = format!("https://shop.example.com/{}", "a".repeat(MAX_URL_BYTES));
    assert_eq!(parse_https(&long), Err(UrlError::Malformed));
}

#[test]
fn a_name_resolving_to_a_private_address_is_refused() {
    // DNS rebinding: a name that answers with a public address first and a
    // private one later is refused as soon as any answer is private.
    let url = parse_https("https://shop.example.com/hook").unwrap();
    let answers = [ip("93.184.216.34"), ip("10.0.0.5")];
    assert_eq!(pin(url, &answers).map(|_| ()), Err(UrlError::NotPublic));
}

#[test]
fn a_name_that_does_not_resolve_is_refused() {
    let url = parse_https("https://shop.example.com/hook").unwrap();
    assert_eq!(pin(url, &[]).map(|_| ()), Err(UrlError::NotPublic));
}

#[test]
fn a_public_name_is_pinned_to_its_approved_addresses() {
    let url = parse_https("https://shop.example.com/hook").unwrap();
    let target = pin(url, &[ip("93.184.216.34")]).unwrap();
    assert_eq!(target.addrs.len(), 1);
    assert_eq!(target.addrs[0].port(), 443);
    assert_eq!(target.addrs[0].ip(), ip("93.184.216.34"));
}

#[test]
fn public_ip_table() {
    for public in ["8.8.8.8", "93.184.216.34", "2001:4860:4860::8888"] {
        assert!(is_public_ip(ip(public)), "{public} should be public");
    }
    for private in [
        "0.1.2.3",
        "127.0.0.1",
        "10.0.0.1",
        "172.31.255.255",
        "192.168.0.1",
        "100.127.0.1",
        "169.254.0.1",
        "192.0.2.1",
        "198.18.0.1",
        "224.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "2001:db8::1",
        "::ffff:10.0.0.1",
        "64:ff9b::7f00:1",
        "2002:7f00:1::1",
        "fec0::1",
        "2001::1",
        "2001:10::1",
        "64:ff9b:1::1",
        "192.88.99.1",
    ] {
        assert!(!is_public_ip(ip(private)), "{private} should be refused");
    }
}

#[test]
fn only_the_https_default_port_is_allowed() {
    // Any other port turns the webhook into a port scanner aimed at public hosts.
    assert_eq!(parse_https("https://shop.example:8443/hook").map(|_| ()), Err(UrlError::Port));
    assert_eq!(parse_https("https://shop.example:80/hook").map(|_| ()), Err(UrlError::Port));
    assert!(parse_https("https://shop.example:443/hook").is_ok());
    assert!(parse_https("https://shop.example/hook").is_ok());
}
