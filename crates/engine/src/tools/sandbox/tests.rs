use std::sync::Arc;

use serde_json::json;
use shared::ports::sandbox::ExecOutput;

use super::*;
use crate::testing::FakeSandbox;

fn call(name: &str, input: Value) -> ToolCall {
    ToolCall { id: "sevt_1".into(), name: name.into(), input, thread: "sthr_1".into() }
}

fn tools(sandbox: &FakeSandbox) -> SandboxTools {
    SandboxTools::new(Arc::new(sandbox.clone()), "sesn_1", WebTools::default())
}

#[tokio::test]
async fn bash_runs_in_the_thread_shell_and_reports_a_failing_exit() {
    let sandbox = FakeSandbox::new(|_| ExecOutput {
        exit_code: 2,
        stdout: b"out\n".to_vec(),
        stderr: b"err\n".to_vec(),
        timed_out: false,
    });
    let outcome = tools(&sandbox).execute(&call("bash", json!({"command": "ls /nope"}))).await;
    assert_eq!(outcome, ToolOutcome::ok("out\nerr\n\nExit code: 2"));
    let exec = &sandbox.execs()[0];
    assert_eq!(exec.workdir.as_deref(), Some(WORKDIR));
    assert!(exec.argv[4].contains("ls /nope"));
    assert!(exec.argv[4].contains("/tmp/.agentd/shell-sthr_1.cwd"));
}

#[tokio::test]
async fn read_numbers_lines_and_honours_view_range() {
    let sandbox = FakeSandbox::answering("a\nb\nc\n");
    let t = tools(&sandbox);
    assert_eq!(
        t.execute(&call("read", json!({"file_path": "/f"}))).await,
        ToolOutcome::ok("1\ta\n2\tb\n3\tc\n")
    );
    let ranged = t.execute(&call("read", json!({"file_path": "/f", "view_range": [2, -1]}))).await;
    assert_eq!(ranged, ToolOutcome::ok("2\tb\n3\tc\n"));
    assert_eq!(sandbox.execs()[0].argv.last().unwrap(), "/f");
}

#[tokio::test]
async fn write_sends_the_content_on_stdin_and_says_whether_it_created() {
    let sandbox = FakeSandbox::answering("0\n");
    let outcome = tools(&sandbox)
        .execute(&call("write", json!({"file_path": "/mnt/session/outputs/a.json", "content": "{}"})))
        .await;
    assert_eq!(outcome, ToolOutcome::ok("File created: /mnt/session/outputs/a.json"));
    assert_eq!(sandbox.execs()[0].stdin.as_deref(), Some(b"{}".as_slice()));
}

#[tokio::test]
async fn edit_rewrites_the_file_with_one_replacement() {
    let sandbox = FakeSandbox::new(|req| {
        let stdout = if req.stdin.is_some() { b"1".to_vec() } else { b"name = old\n".to_vec() };
        ExecOutput { stdout, ..ExecOutput::default() }
    });
    let outcome = tools(&sandbox)
        .execute(&call("edit", json!({"file_path": "/w/f", "old_string": "old", "new_string": "new"})))
        .await;
    assert_eq!(outcome, ToolOutcome::ok("File edited: /w/f (1 replacement)"));
    assert_eq!(sandbox.execs()[1].stdin.as_deref(), Some(b"name = new\n".as_slice()));
}

#[tokio::test]
async fn glob_matches_paths_relative_to_the_base() {
    let sandbox = FakeSandbox::answering("/w/a.json\n/w/sub/b.json\n/w/c.txt\n");
    let t = tools(&sandbox);
    let top = t.execute(&call("glob", json!({"pattern": "*.json", "path": "/w"}))).await;
    assert_eq!(top, ToolOutcome::ok("/w/a.json"));
    let deep = t.execute(&call("glob", json!({"pattern": "**/*.json", "path": "/w"}))).await;
    assert_eq!(deep, ToolOutcome::ok("/w/a.json\n/w/sub/b.json"));
}

#[tokio::test]
async fn grep_with_no_match_says_so() {
    let sandbox = FakeSandbox::new(|_| ExecOutput { exit_code: 1, ..ExecOutput::default() });
    let outcome =
        tools(&sandbox).execute(&call("grep", json!({"pattern": "x", "case_insensitive": true}))).await;
    assert_eq!(outcome, ToolOutcome::ok("No matches found"));
    assert!(sandbox.execs()[0].argv[4].contains("grep -rnEi"));
}

#[tokio::test]
async fn an_oversized_output_is_saved_and_previewed() {
    let big = "x".repeat(text::OUTPUT_LIMIT + 1);
    let sandbox = FakeSandbox::new(move |req| {
        let stdout = if req.stdin.is_some() { b"0".to_vec() } else { big.as_bytes().to_vec() };
        ExecOutput { stdout, ..ExecOutput::default() }
    });
    let outcome = tools(&sandbox).execute(&call("bash", json!({"command": "yes"}))).await;
    assert!(outcome.output.len() < 3000);
    assert!(outcome.output.contains("/tmp/agentd-tool-outputs/sevt_1.txt"));
}

#[tokio::test]
async fn missing_inputs_and_unknown_tools_are_errors_for_the_model() {
    let t = tools(&FakeSandbox::answering(""));
    assert_eq!(t.execute(&call("read", json!({}))).await, ToolOutcome::err("read: `file_path` is required"));
    assert_eq!(t.execute(&call("teleport", json!({}))).await, ToolOutcome::err("unknown tool: teleport"));
}
