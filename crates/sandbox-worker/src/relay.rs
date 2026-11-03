//! `agentd-worker relay`: the gateway between the sandboxes' internal
//! network and agentd on the host. It forwards only the ports it is given
//! (agentd's API and its egress proxy), so a sandbox has no other way out.
//!
//! `AGENTD_RELAY`: `port=host:port,…`.

use tokio::net::{TcpListener, TcpStream};

/// The `(listen port, target)` pairs of `spec`.
pub fn parse(spec: &str) -> Result<Vec<(u16, String)>, String> {
    spec.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|pair| {
            let (port, target) =
                pair.split_once('=').ok_or(format!("AGENTD_RELAY: `{pair}` is not port=host:port"))?;
            let port = port.trim().parse().map_err(|_| format!("AGENTD_RELAY: `{port}` is not a port"))?;
            Ok((port, target.trim().to_owned()))
        })
        .collect()
}

/// Relay every pair until the process ends.
pub async fn run(pairs: Vec<(u16, String)>) {
    let mut tasks = Vec::new();
    for (port, target) in pairs {
        let listener = match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("agentd-worker relay: port {port}: {e}");
                std::process::exit(1);
            }
        };
        tasks.push(tokio::spawn(async move {
            loop {
                let Ok((mut inbound, _)) = listener.accept().await else { continue };
                let target = target.clone();
                tokio::spawn(async move {
                    if let Ok(mut outbound) = TcpStream::connect(&target).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_parse_and_bad_ones_are_refused() {
        let pairs = parse("8080=host.docker.internal:8080, 8890=host.docker.internal:8890").unwrap();
        assert_eq!(
            pairs,
            [(8080, "host.docker.internal:8080".into()), (8890, "host.docker.internal:8890".into())]
        );
        assert!(parse("8080").is_err());
        assert!(parse("x=y:1").is_err());
    }
}
