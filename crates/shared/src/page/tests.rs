use super::*;

#[test]
fn empty_page_matches_live_shape() {
    // Live (July capture): an empty Managed Agents list is {"data":[]}.
    let p: Page<u8> = Page::paginate(vec![], &Pagination { limit: 20, offset: 0 });
    assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"data":[]}"#);
}

#[test]
fn id_cursor_window_bounds_and_has_more() {
    #[allow(clippy::ptr_arg)] // the &String matches window's `Fn(&T)` HRTB
    fn key(s: &String) -> &str {
        s.as_str()
    }
    // Items are ids "0".."9" in list order.
    let all: Vec<String> = (0..10).map(|n| n.to_string()).collect();

    // after_id excludes everything up to and including it; limit caps + flags more.
    let c = IdCursor { after_id: Some("2".into()), after_created: None, before_id: None, limit: 3 };
    let (data, more) = c.window(all.clone(), key);
    assert_eq!(data, vec!["3", "4", "5"]);
    assert!(more);

    // before_id is an exclusive upper bound.
    let c =
        IdCursor { after_id: Some("2".into()), after_created: None, before_id: Some("6".into()), limit: 10 };
    let (data, more) = c.window(all.clone(), key);
    assert_eq!(data, vec!["3", "4", "5"]);
    assert!(!more);

    // Unknown cursor id is a no-op bound (whole set, from the start).
    let c = IdCursor { after_id: Some("nope".into()), after_created: None, before_id: None, limit: 100 };
    let (data, more) = c.window(all, key);
    assert_eq!(data.len(), 10);
    assert!(!more);
}

#[test]
fn from_fetched_matches_paginate_over_full_set() {
    // The store fetches `fetch_limit()` rows at `sql_offset()`; from_fetched
    // must produce the same page (data + next_page) as slicing the full set.
    let all: Vec<u8> = (0..25).collect();
    for offset in [0usize, 10, 20, 25, 40] {
        for limit in [1usize, 7, 20, 100] {
            let page = Pagination { limit, offset };
            let full = Page::paginate(all.clone(), &page);
            // Simulate the DB fetch: skip=offset, take=limit+1.
            let fetched: Vec<u8> = all
                .iter()
                .copied()
                .skip(page.sql_offset() as usize)
                .take(page.fetch_limit() as usize)
                .collect();
            let db = Page::from_fetched(fetched, &page);
            assert_eq!(db.data, full.data, "data at off={offset} lim={limit}");
            assert_eq!(db.next_page, full.next_page, "cursor at off={offset} lim={limit}");
        }
    }
}

#[test]
fn paginate_sets_next_page_only_when_more() {
    let all: Vec<u8> = (0..25).collect();
    let first = Page::paginate(all.clone(), &Pagination { limit: 20, offset: 0 });
    assert_eq!(first.data.len(), 20);
    let cursor = first.next_page.clone().expect("more results → cursor present");
    assert!(cursor.starts_with("page_"));

    // round-trip the cursor to fetch page 2
    let mut params = HashMap::new();
    params.insert("page".to_string(), cursor);
    let p2 = Pagination::from_params(&params).unwrap();
    let second = Page::paginate(all, &p2);
    assert_eq!(second.data, (20..25).collect::<Vec<u8>>());
    assert!(second.next_page.is_none(), "last page omits next_page");
}

#[test]
fn limit_validation_matches_live_message() {
    let mut params = HashMap::new();
    params.insert("limit".to_string(), "0".to_string());
    let err = Pagination::from_params(&params).unwrap_err();
    assert_eq!(err.message, "limit: value must be greater than or equal to 1 and less than or equal to 100");
    params.insert("limit".to_string(), "101".to_string());
    assert!(Pagination::from_params(&params).is_err());
    params.insert("limit".to_string(), "100".to_string());
    assert_eq!(Pagination::from_params(&params).unwrap().limit, 100);
}

#[test]
fn the_beta_flag_passes_and_an_unknown_param_still_fails() {
    // Live: the SDKs and `ant` send `?beta=true` on every beta call.
    let params: HashMap<String, String> = [("beta".to_string(), "true".to_string())].into();
    assert!(reject_unknown_params(&params, &["version"]).is_ok());
    let params: HashMap<String, String> = [("bogus".to_string(), "1".to_string())].into();
    assert_eq!(
        reject_unknown_params(&params, &["version"]).unwrap_err().message,
        "Failed to parse request body: unknown field \"bogus\""
    );
}
