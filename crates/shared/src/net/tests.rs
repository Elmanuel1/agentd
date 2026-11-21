use super::*;

#[test]
fn every_internal_range_is_refused_and_public_ones_pass() {
    for bad in [
        "10.0.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "0.1.2.3",
        "192.0.0.8",
        "198.18.0.1",
        "240.0.0.1",
        "224.0.0.1",
        "::1",
        "fe80::1",
        "fd00::1",
        "::ffff:169.254.169.254",
        "::169.254.169.254",
        "64:ff9b::a9fe:a9fe",
        "64:ff9b:1::1",
        "2002:a9fe:a9fe::1",
        "ff02::1",
    ] {
        assert!(internal(bad.parse().unwrap()), "{bad}");
    }
    for good in ["8.8.8.8", "140.82.112.3", "2606:4700::1111", "2002:808:808::1"] {
        assert!(!internal(good.parse().unwrap()), "{good}");
    }
}

#[test]
fn a_wildcard_rule_matches_subdomains_only() {
    assert!(host_matches("api.example.com", "API.example.com."));
    assert!(host_matches("*.example.com", "a.example.com"));
    assert!(!host_matches("*.example.com", "example.com"));
    assert!(!host_matches("example.com", "a.example.com"));
}

#[tokio::test]
async fn a_loopback_url_is_refused_unless_its_socket_is_allowed() {
    let url: reqwest::Url = "http://127.0.0.1:9/mcp".parse().unwrap();
    assert_eq!(Guard::default().resolve(&url).await, Err(NetError::Internal("127.0.0.1".into())));
    let allowed = Guard::allowing(vec!["127.0.0.1:9".parse().unwrap()]);
    assert!(allowed.resolve(&url).await.is_ok());
    let other: reqwest::Url = "http://127.0.0.1:10/mcp".parse().unwrap();
    assert!(allowed.resolve(&other).await.is_err());
    let metadata: reqwest::Url = "http://169.254.169.254/latest".parse().unwrap();
    assert!(matches!(allowed.resolve(&metadata).await, Err(NetError::Internal(_))));
}

/// The guard's client for `url`, and whether it came from the cache.
async fn cached(guard: &Guard, url: &reqwest::Url) -> bool {
    let key = (url.host_str().unwrap().to_owned(), url.port().unwrap(), TIMEOUT, guard.user_agent);
    let hit = guard.clients.get(&key).is_some();
    guard.client(url, TIMEOUT).await.unwrap();
    hit
}

const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test(start_paused = true)]
async fn two_calls_to_one_host_reuse_a_client_and_after_5_min_the_address_is_checked_again() {
    let url: reqwest::Url = "http://127.0.0.1:9/mcp".parse().unwrap();
    let guard = Guard::allowing(vec!["127.0.0.1:9".parse().unwrap()]);
    assert!(!cached(&guard, &url).await);
    let other: reqwest::Url = "http://127.0.0.1:9/other".parse().unwrap();
    assert!(cached(&guard.clone(), &other).await, "a clone shares the cache");
    tokio::time::advance(CLIENT_TTL).await;
    assert!(!cached(&guard, &url).await, "resolved and checked again");
    let platform = guard.with_user_agent("Claude-Platform");
    assert!(!cached(&platform, &url).await, "another user agent builds its own");
}
