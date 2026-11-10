//! The `agentd keys`, `agentd admin-keys` and `agentd environment-keys`
//! commands' shared words: `create` with its flags, `list` and `revoke`, and
//! how each runs and exits.

use std::process::ExitCode;

use shared::{with_writer, Writer};

/// Which key a command makes: the flags its `create` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A workspace key: `--org` and `--workspace`.
    Workspace,
    /// An admin key: `--org` only.
    Admin,
    /// An environment key: `--org`, `--workspace` and `--environment`.
    Environment,
}

/// One key command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysCommand {
    /// Issue a key for an org and, as its kind needs, a workspace and an environment.
    Create {
        /// The org id.
        org: String,
        /// The workspace id; `None` for an admin key.
        workspace: Option<String>,
        /// The environment id; set for an environment key only.
        environment: Option<String>,
        /// A label for people.
        name: String,
    },
    /// Show every key, without its secret.
    List,
    /// Revoke a key by its id.
    Revoke(String),
}

impl KeysCommand {
    /// The command in `args`, the words after the command's name.
    pub fn parse(args: &[String], kind: Kind) -> Result<Self, String> {
        match args {
            [cmd, rest @ ..] if cmd == "create" => Self::create(rest, kind),
            [cmd] if cmd == "list" => Ok(Self::List),
            [cmd, id] if cmd == "revoke" => Ok(Self::Revoke(id.clone())),
            _ => Err("unknown keys command".into()),
        }
    }

    fn create(rest: &[String], kind: Kind) -> Result<Self, String> {
        let (mut org, mut workspace, mut environment, mut name) = (None, None, None, None);
        let mut flags = rest.iter();
        while let Some(flag) = flags.next() {
            let slot = match (flag.as_str(), kind) {
                ("--org", _) => &mut org,
                ("--workspace", Kind::Workspace | Kind::Environment) => &mut workspace,
                ("--environment", Kind::Environment) => &mut environment,
                ("--name", _) => &mut name,
                (other, _) => return Err(format!("unknown option `{other}`")),
            };
            *slot = Some(flags.next().ok_or_else(|| format!("{flag} needs a value"))?.clone());
        }
        if kind != Kind::Admin && workspace.is_none() {
            return Err("--workspace is required".into());
        }
        if kind == Kind::Environment && environment.is_none() {
            return Err("--environment is required".into());
        }
        let org = org.ok_or("--org is required")?;
        Ok(Self::Create { org, workspace, environment, name: name.unwrap_or_else(|| "agentd".into()) })
    }
}

/// Run `fut` as the CLI's writer, `operator`, in a request of its own.
pub async fn as_operator<F: std::future::Future>(fut: F) -> F::Output {
    with_writer(Writer::fresh("operator"), fut).await
}

/// Parse `args` for `kind` and run the command with `body` as the CLI's
/// writer, `operator`; errors go to stderr as `agentd <name>: …`.
pub async fn run<F, Fut>(name: &str, usage: &str, kind: Kind, args: &[String], body: F) -> ExitCode
where
    F: FnOnce(KeysCommand) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let command = match KeysCommand::parse(args, kind) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{usage}");
            return ExitCode::FAILURE;
        }
    };
    match as_operator(body(command)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentd {name}: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &str) -> Vec<String> {
        words.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn create_takes_an_org_a_workspace_and_an_optional_name() {
        assert_eq!(
            KeysCommand::parse(&args("create --workspace wrkspc_a --org org_a"), Kind::Workspace),
            Ok(KeysCommand::Create {
                org: "org_a".into(),
                workspace: Some("wrkspc_a".into()),
                environment: None,
                name: "agentd".into()
            })
        );
        assert!(matches!(
            KeysCommand::parse(&args("create --org o --workspace w --name ci"), Kind::Workspace),
            Ok(KeysCommand::Create { name, .. }) if name == "ci"
        ));
        for bad in ["create", "create --org o", "create --org o --workspace", "create --org o --key k"] {
            assert!(KeysCommand::parse(&args(bad), Kind::Workspace).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_admin_key_takes_an_org_and_no_workspace() {
        assert_eq!(
            KeysCommand::parse(&args("create --org org_a"), Kind::Admin),
            Ok(KeysCommand::Create {
                org: "org_a".into(),
                workspace: None,
                environment: None,
                name: "agentd".into()
            })
        );
        for bad in ["create", "create --org o --workspace w"] {
            assert!(KeysCommand::parse(&args(bad), Kind::Admin).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_environment_key_takes_an_org_a_workspace_and_an_environment() {
        assert_eq!(
            KeysCommand::parse(
                &args("create --environment env_a --workspace wrkspc_a --org org_a"),
                Kind::Environment
            ),
            Ok(KeysCommand::Create {
                org: "org_a".into(),
                workspace: Some("wrkspc_a".into()),
                environment: Some("env_a".into()),
                name: "agentd".into()
            })
        );
        for bad in ["create --org o --workspace w", "create --org o --environment e", "create --x y"] {
            assert!(KeysCommand::parse(&args(bad), Kind::Environment).is_err(), "{bad}");
        }
        assert!(KeysCommand::parse(&args("create --org o --workspace w --environment e"), Kind::Workspace)
            .is_err());
    }

    #[test]
    fn list_and_revoke_take_what_they_need() {
        for admin in [Kind::Workspace, Kind::Admin, Kind::Environment] {
            assert_eq!(KeysCommand::parse(&args("list"), admin), Ok(KeysCommand::List));
            assert_eq!(
                KeysCommand::parse(&args("revoke apikey_01a"), admin),
                Ok(KeysCommand::Revoke("apikey_01a".into()))
            );
            for bad in ["", "list x", "revoke", "revoke a b", "rotate"] {
                assert!(KeysCommand::parse(&args(bad), admin).is_err(), "{bad}");
            }
        }
    }
}
