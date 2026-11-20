//! The TLS parameters of a libpq connection string, `sslmode` and
//! `sslrootcert`, taken out of it in either form (`postgres://…?sslmode=…` or
//! `host=… sslmode=…`): tokio-postgres reads every other parameter but knows
//! neither `sslrootcert` nor the `verify-*` modes.

use std::path::PathBuf;

use percent_encoding::percent_decode_str;

/// libpq's `sslmode` (<https://www.postgresql.org/docs/current/libpq-ssl.html>).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

/// `sslrootcert`: a PEM file, or `system` for the operating system's roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RootCert {
    File(PathBuf),
    System,
}

/// A connection string without its TLS parameters, and those parameters.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ConnInfo {
    pub(crate) rest: String,
    pub(crate) mode: SslMode,
    pub(crate) root: Option<RootCert>,
}

const MODES: &str = "disable, prefer, require, verify-ca or verify-full";

/// Split `url`. With a root certificate, `require` checks the chain as
/// `verify-ca` does, as libpq's does.
pub(crate) fn split(url: &str) -> Result<ConnInfo, String> {
    let (rest, pairs) = if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        split_url(url)
    } else {
        split_keywords(url)?
    };
    let mut mode = SslMode::Prefer;
    let mut root = None;
    for (key, value) in pairs {
        match key.as_str() {
            "sslmode" => mode = parse_mode(&value)?,
            _ if value == "system" => root = Some(RootCert::System),
            _ => root = Some(RootCert::File(PathBuf::from(value))),
        }
    }
    let mode = if mode == SslMode::Require && root.is_some() { SslMode::VerifyCa } else { mode };
    Ok(ConnInfo { rest, mode, root })
}

fn parse_mode(value: &str) -> Result<SslMode, String> {
    match value {
        "disable" => Ok(SslMode::Disable),
        "prefer" => Ok(SslMode::Prefer),
        "require" => Ok(SslMode::Require),
        "verify-ca" => Ok(SslMode::VerifyCa),
        "verify-full" => Ok(SslMode::VerifyFull),
        other => Err(format!("sslmode `{other}` is not supported: use {MODES}")),
    }
}

fn is_tls_key(key: &str) -> bool {
    key == "sslmode" || key == "sslrootcert"
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn split_url(url: &str) -> (String, Vec<(String, String)>) {
    let Some((base, query)) = url.split_once('?') else {
        return (url.to_owned(), Vec::new());
    };
    let (kept, taken): (Vec<&str>, Vec<&str>) = query
        .split('&')
        .filter(|p| !p.is_empty())
        .partition(|p| !is_tls_key(&decode(p.split_once('=').map_or(*p, |(k, _)| k))));
    let pairs = taken
        .iter()
        .map(|p| p.split_once('=').map_or((decode(p), String::new()), |(k, v)| (decode(k), decode(v))))
        .collect();
    let rest = if kept.is_empty() { base.to_owned() } else { format!("{base}?{}", kept.join("&")) };
    (rest, pairs)
}

/// libpq's `keyword = value` form: a value is single-quoted or runs to the
/// next space, and a backslash escapes the character after it in either.
fn split_keywords(conninfo: &str) -> Result<(String, Vec<(String, String)>), String> {
    let mut chars = conninfo.char_indices().peekable();
    let mut kept = Vec::new();
    let mut pairs = Vec::new();
    loop {
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        let Some(&(start, _)) = chars.peek() else { break };
        let mut key = String::new();
        while let Some((_, c)) = chars.next_if(|(_, c)| *c != '=' && !c.is_whitespace()) {
            key.push(c);
        }
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        if chars.next_if(|(_, c)| *c == '=').is_none() {
            return Err(format!("the connection string has no `=` after `{key}`"));
        }
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        let value = keyword_value(&mut chars)?;
        let end = chars.peek().map_or(conninfo.len(), |(i, _)| *i);
        match is_tls_key(&key) {
            true => pairs.push((key, value)),
            false => kept.push(conninfo.get(start..end).unwrap_or_default().trim_end()),
        }
    }
    Ok((kept.join(" "), pairs))
}

fn keyword_value(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> Result<String, String> {
    let quoted = chars.next_if(|(_, c)| *c == '\'').is_some();
    let mut value = String::new();
    while let Some((_, c)) = chars.next_if(|(_, c)| quoted || !c.is_whitespace()) {
        match c {
            '\\' => value.extend(chars.next().map(|(_, c)| c)),
            '\'' if quoted => return Ok(value),
            c => value.push(c),
        }
    }
    match quoted {
        true => Err("the connection string has an unterminated quoted value".into()),
        false => Ok(value),
    }
}

#[cfg(test)]
mod tests;
