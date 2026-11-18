//! Key rings, digests and their rotation.

use super::*;

#[test]
fn matches_live_fixture_hash() {
    // Live probe: content "v1 content" (10 bytes) →
    // "60329bc85480d3c6a566d6101ce5f9fbee836b89e1952cf16326fb392cb59c55"
    assert_eq!(
        content_sha256(b"v1 content"),
        "60329bc85480d3c6a566d6101ce5f9fbee836b89e1952cf16326fb392cb59c55"
    );
}

#[test]
fn is_lowercase_hex_64() {
    let h = content_sha256(b"anything");
    assert_eq!(h.len(), 64);
    assert!(h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
}

#[test]
fn hmac_matches_rfc_4231_case_2() {
    let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    assert_eq!(hex::encode(mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
}

#[test]
fn a_token_verifies_only_unchanged_under_its_signer_and_purpose() {
    let s = Signer::new(KeyRing::single(b"k".to_vec()));
    let t = s.sign("x-", &vec!["a"]);
    assert_eq!(s.verify::<Vec<String>>("x-", &t), Some(vec!["a".to_owned()]));
    assert_eq!(s.derive("other").verify::<Vec<String>>("x-", &t), None);
    assert_eq!(s.verify::<Vec<String>>("y-", &t), None);
    assert_eq!(s.verify::<Vec<String>>("x-", &t.replace(".", ".A")), None);
}

#[test]
fn a_token_names_its_key_and_verifies_while_that_key_is_in_the_ring() {
    let old = KeyRing::parse("k1:000102030405060708090a0b0c0d0e0f").unwrap();
    let rotated =
        KeyRing::parse("k2:101112131415161718191a1b1c1d1e1f,k1:000102030405060708090a0b0c0d0e0f").unwrap();
    let t = Signer::new(old).sign("x-", &vec!["a"]);
    assert!(t.starts_with("x-k1."));
    let next = Signer::new(rotated);
    assert_eq!(
        next.verify::<Vec<String>>("x-", &t),
        Some(vec!["a".to_owned()]),
        "the old key still verifies"
    );
    assert!(next.sign("x-", &vec!["a"]).starts_with("x-k2."), "the new key signs");
    let retired = Signer::new(KeyRing::parse("k2:101112131415161718191a1b1c1d1e1f").unwrap());
    assert_eq!(retired.verify::<Vec<String>>("x-", &t), None);
    assert!(
        KeyRing::parse("k1:00").is_err()
            && KeyRing::parse("").is_err()
            && KeyRing::parse("k-1:000102030405060708090a0b0c0d0e0f").is_err()
    );
}

#[test]
fn a_bad_entry_is_named_by_its_position_never_its_text() {
    let key = "000102030405060708090a0b0c0d0e0f";
    for raw in [format!("{key}:k1"), format!("k1:{key},k2{key}"), format!("k1:{key},k2:00")] {
        let err = KeyRing::parse(&raw).unwrap_err().to_string();
        assert!(err.starts_with("entry ") && !err.contains("0001"), "{err}");
    }
    assert_eq!(
        KeyRing::parse(&format!("k1:{key},{key}:k2")).unwrap_err().to_string(),
        "entry 2 is not `<id>:<hex of at least 16 bytes>`"
    );
    assert_eq!(
        KeyRing::parse(&format!("k1:{key},k2:{key},k1:{key}")).unwrap_err().to_string(),
        "entry 3 repeats an earlier id"
    );
    for (raw, want) in [
        (format!("k1:{key},,k2:00"), "entry 3 is not `<id>:<hex of at least 16 bytes>`"),
        (format!(",k1:{key}, ,k1:{key}"), "entry 4 repeats an earlier id"),
    ] {
        assert_eq!(KeyRing::parse(&raw).unwrap_err().to_string(), want, "an empty field counts");
    }
}
