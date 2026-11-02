use super::*;

#[test]
fn allowed_hosts_follow_the_documented_matching() {
    let web = WebTools {
        allowed_hosts: Some(vec!["example.com".into(), "*.api.io".into()]),
        ..WebTools::default()
    };
    assert!(web.host_allowed("example.com"));
    assert!(!web.host_allowed("www.example.com"));
    assert!(web.host_allowed("v1.api.io"));
    assert!(!web.host_allowed("api.io"));
    assert!(WebTools::default().host_allowed("anything.net"));
}

#[tokio::test]
async fn internal_addresses_are_refused() {
    assert!(public_only("127.0.0.1", 80).await.is_err());
    assert!(public_only("10.1.2.3", 80).await.is_err());
}

#[test]
fn html_keeps_only_visible_text() {
    let t = strip_tags("<html><style>x{}</style><body><p>Hi</p>\n<script>evil()</script><b>there</b></body>");
    assert_eq!(t, "Hi\nthere");
}
