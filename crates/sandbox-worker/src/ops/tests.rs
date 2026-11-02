use std::collections::BTreeMap;

use sandbox_protocol::{decode, encode, FileOut, Op, Outcome};

use super::*;

fn me() -> Agent {
    // Tests run unprivileged: commands run as the current user.
    let out = std::process::Command::new("id").arg("-u").output().unwrap();
    let uid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    let out = std::process::Command::new("id").arg("-g").output().unwrap();
    Agent { uid, gid: String::from_utf8_lossy(&out.stdout).trim().parse().unwrap() }
}

#[tokio::test]
async fn a_command_gets_stdin_and_a_clean_environment() {
    std::env::set_var("AGENTD_WORK_SECRET", "must-not-leak");
    let op = Op::Exec {
        argv: vec!["sh".into(), "-c".into(), "cat; echo \"[$AGENTD_WORK_SECRET][$EXTRA]\"; exit 3".into()],
        stdin: Some(encode(b"in\n")),
        workdir: None,
        timeout_ms: 5000,
        env: BTreeMap::from([("EXTRA".into(), "x".into())]),
    };
    let Outcome::Exec { exit_code, stdout, timed_out, .. } = run(op, me()).await else { panic!() };
    assert_eq!(String::from_utf8(decode(&stdout).unwrap()).unwrap(), "in\n[][x]\n");
    assert_eq!((exit_code, timed_out), (3, false));
}

#[tokio::test]
async fn a_command_past_its_timeout_is_killed() {
    let op = Op::Exec {
        argv: vec!["sleep".into(), "5".into()],
        stdin: None,
        workdir: None,
        timeout_ms: 200,
        env: BTreeMap::new(),
    };
    let Outcome::Exec { timed_out, .. } = run(op, me()).await else { panic!() };
    assert!(timed_out);
}

#[tokio::test]
async fn files_round_trip_through_a_tree_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a/b.json").to_string_lossy().into_owned();
    let put = Op::PutFiles { files: vec![FileOut { path: path.clone(), bytes: encode(b"{}"), mode: 0o644 }] };
    // chown to root needs root; unprivileged, the write itself is what we check.
    let _ = run(put, me()).await;
    let Outcome::Tree { files } =
        run(Op::ReadTree { dir: dir.path().to_string_lossy().into_owned() }, me()).await
    else {
        panic!()
    };
    assert_eq!(decode(&files[&path]).unwrap(), b"{}");
}
