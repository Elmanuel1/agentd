//! `agentd organization create|delete`: the one way to make an organization, and to remove it with
//! everything it owns. A created organization's id is printed alone on stdout; everything else goes
//! to stderr.

use std::process::ExitCode;
use std::sync::Arc;

use organizations::deletion::{Deleted, OrganizationDeletion};
use organizations::registry::{OrganizationsStore, new_id};
use shared::OrgId;
use shared::charset::is_id_charset;
use store_postgres::PgOrganizationsStore;

use super::command::as_operator;
use crate::backends::outside;
use crate::backends::postgres::primary;
use crate::config::Config;

/// How to call it.
pub const USAGE: &str = "usage: agentd organization create [--id <org_id>]
       agentd organization delete <org_id>";

/// One organization command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrganizationCommand {
    /// Make an organization, with the id given or a generated `org_…` one.
    Create(Option<String>),
    /// Delete an organization and everything it owns.
    Delete(String),
}

impl OrganizationCommand {
    /// The command in `args`, the words after `agentd organization`.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let id = |raw: &String| match is_id_charset(raw) {
            true => Ok(raw.clone()),
            false => Err(format!("{raw} is not in the id charset")),
        };
        match args {
            [cmd] if cmd == "create" => Ok(Self::Create(None)),
            [cmd, flag, raw] if cmd == "create" && flag == "--id" => id(raw).map(|r| Self::Create(Some(r))),
            [cmd, raw] if cmd == "delete" => id(raw).map(Self::Delete),
            _ => Err("unknown organization command".into()),
        }
    }
}

/// Run `args` (the words after `agentd organization`) against `config`'s backends.
pub async fn organization(config: &Config, args: &[String]) -> ExitCode {
    let command = match OrganizationCommand::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let (lines, code) = as_operator(run(config, command))
        .await
        .unwrap_or_else(|e| (vec![format!("agentd organization: {e}")], ExitCode::FAILURE));
    for line in lines {
        eprintln!("{line}");
    }
    code
}

async fn run(config: &Config, command: OrganizationCommand) -> Result<(Vec<String>, ExitCode), String> {
    let pg = primary(&config.store).await.map_err(|e| e.to_string())?;
    let store = Arc::new(PgOrganizationsStore::new(pg.clone()));
    match command {
        OrganizationCommand::Create(requested) => {
            let org = new_id(requested.as_deref());
            store.create(&org).await.map_err(|e| e.to_string())?;
            println!("{}", org.as_str());
            Ok((vec![format!("created organization {}", org.as_str())], ExitCode::SUCCESS))
        }
        OrganizationCommand::Delete(raw) => {
            let (secrets, blobs, scheduler) =
                outside::stores(config, &pg).await.map_err(|e| e.to_string())?;
            let deletion = OrganizationDeletion::new(store, secrets, blobs, scheduler);
            let deleted = deletion.delete(&OrgId::new(raw)).await.map_err(|e| e.to_string())?;
            Ok(report(&deleted))
        }
    }
}

/// What a delete removed, then what it left; a failure while anything is left.
fn report(deleted: &Deleted) -> (Vec<String>, ExitCode) {
    let removed = &deleted.removed;
    let mut lines = vec![format!("deleted the organization's workspaces: {}", removed.workspaces.join(" "))];
    lines
        .extend(removed.rows.iter().filter(|(_, n)| *n > 0).map(|(table, n)| format!("deleted {n} {table}")));
    lines.extend(deleted.gone.iter().map(|g| format!("removed {g}")));
    let code = match deleted.left.is_empty() {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    };
    lines.extend(deleted.left.iter().map(|l| format!("not removed: {l}")));
    (lines, code)
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use organizations::deletion::Deleted;
    use organizations::registry::Removed;
    use shared::ports::FireTarget;
    use shared::ports::secrets::SecretKey;

    use super::{OrganizationCommand, report};

    fn deleted(left: &[&str]) -> Deleted {
        let removed = Removed {
            workspaces: vec!["wrkspc_a".into(), "wrkspc_b".into()],
            rows: vec![("credentials", 1), ("agents", 0)],
            secrets: vec![SecretKey::new("wrkspc_a", "vlt_a", "vcrd_a").unwrap()],
            blobs: vec!["wrkspc_a/file_a".into()],
            dreams: vec![FireTarget::new("dream", "drm_a")],
        };
        let gone = ["secret wrkspc_a/vlt_a/vcrd_a", "blob wrkspc_a/file_a", "schedule dream/drm_a"];
        let (gone, left): (Vec<String>, Vec<String>) = match left {
            [] => (gone.iter().map(|g| (*g).to_owned()).collect(), vec![]),
            _ => (
                vec!["secret wrkspc_a/vlt_a/vcrd_a".to_owned(), "schedule dream/drm_a".to_owned()],
                left.iter().map(|l| (*l).to_owned()).collect(),
            ),
        };
        Deleted { removed, gone, left }
    }

    #[test]
    fn a_delete_prints_what_it_removed_and_fails_while_anything_is_left() {
        let (lines, code) = report(&deleted(&[]));
        assert_eq!(
            lines,
            [
                "deleted the organization's workspaces: wrkspc_a wrkspc_b",
                "deleted 1 credentials",
                "removed secret wrkspc_a/vlt_a/vcrd_a",
                "removed blob wrkspc_a/file_a",
                "removed schedule dream/drm_a",
            ]
        );
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        let (lines, code) = report(&deleted(&["blob wrkspc_a/file_a: blob backend: refused"]));
        assert_eq!(
            lines,
            [
                "deleted the organization's workspaces: wrkspc_a wrkspc_b",
                "deleted 1 credentials",
                "removed secret wrkspc_a/vlt_a/vcrd_a",
                "removed schedule dream/drm_a",
                "not removed: blob wrkspc_a/file_a: blob backend: refused",
            ]
        );
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn create_takes_an_optional_id_and_delete_one_id() {
        assert_eq!(OrganizationCommand::parse(&args(&["create"])), Ok(OrganizationCommand::Create(None)));
        assert_eq!(
            OrganizationCommand::parse(&args(&["create", "--id", "org_sdk"])),
            Ok(OrganizationCommand::Create(Some("org_sdk".into())))
        );
        assert_eq!(
            OrganizationCommand::parse(&args(&["delete", "org_sdk"])),
            Ok(OrganizationCommand::Delete("org_sdk".into()))
        );
        assert!(OrganizationCommand::parse(&args(&["create", "--id"])).is_err());
        assert!(OrganizationCommand::parse(&args(&["delete", "org/../x"])).is_err());
        assert!(OrganizationCommand::parse(&args(&["rename", "org_sdk"])).is_err());
    }
}
