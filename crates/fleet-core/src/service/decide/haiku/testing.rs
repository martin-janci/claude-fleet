//! A scripted transport for the benchmarks' haiku tests: it reads the
//! prompt from each call's stdin, as the host's `claude` would receive it,
//! checks the command line holds none of it, and answers with whatever the
//! test's function returns.

use super::*;
use crate::ipc_error::IpcError;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The state JSON in a prompt.
pub fn state_of(prompt: &str) -> Option<Value> {
    let a = prompt.find("<state>\n")? + "<state>\n".len();
    let b = prompt[a..].find("\n</state>")? + a;
    serde_json::from_str(&prompt[a..b]).ok()
}

/// What `claude -p --output-format json` prints for a model `text`.
pub fn envelope(text: &str) -> String {
    serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": text,
        "total_cost_usd": 0.0005,
        "usage": { "input_tokens": 10, "cache_read_input_tokens": 90, "output_tokens": 5 },
    })
    .to_string()
}

type Answer = Box<dyn Fn(&str) -> String + Send + Sync>;

/// Answers each call from its prompt with `answer`'s model text (wrapped in
/// an envelope), counting calls and keeping every prompt and host.
pub struct ScriptedSsh {
    answer: Answer,
    pub calls: AtomicUsize,
    pub prompts: Mutex<Vec<String>>,
    pub hosts: Mutex<Vec<String>>,
}

impl ScriptedSsh {
    pub fn new(answer: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        ScriptedSsh {
            answer: Box::new(answer),
            calls: AtomicUsize::new(0),
            prompts: Mutex::new(Vec::new()),
            hosts: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl SshExec for ScriptedSsh {
    async fn run(&self, _: &str, _: &[&str], _: Duration) -> Result<Output, IpcError> {
        unreachable!("haiku sends its prompt on stdin")
    }

    async fn run_bounded(
        &self,
        _: &str,
        _: &[&str],
        _: Duration,
        _: Duration,
    ) -> Result<Output, IpcError> {
        unreachable!("haiku sends its prompt on stdin")
    }

    async fn run_with_stdin(
        &self,
        host: &str,
        args: &[&str],
        stdin: Vec<u8>,
        _connect: Duration,
        _wall: Duration,
        _max_output: usize,
    ) -> Result<Output, IpcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.hosts.lock().unwrap().push(host.to_string());
        let script = match args {
            ["bash", "-lc", s] => crate::ssh_fake::unquote(s).expect("a quoted script"),
            _ => panic!("not a bash -lc call: {args:?}"),
        };
        let prompt = String::from_utf8(stdin).expect("a UTF-8 prompt");
        let state = prompt
            .split("<state>\n")
            .nth(1)
            .and_then(|x| x.split("\n</state>").next())
            .expect("a state in the prompt");
        assert!(
            !script.contains(state) && !script.contains(PREAMBLE),
            "the prompt is on the command line: {script}"
        );
        let text = (self.answer)(&prompt);
        self.prompts.lock().unwrap().push(prompt);
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: format!("{HAIKU_TAG}run\n{}\n", envelope(&text)).into_bytes(),
            stderr: Vec::new(),
        })
    }

    async fn run_cancellable(
        &self,
        _: &str,
        _: &[&str],
        _: Duration,
        _: tokio_util::sync::CancellationToken,
    ) -> Result<Output, IpcError> {
        unreachable!()
    }

    async fn run_bounded_cancellable(
        &self,
        _: &str,
        _: &[&str],
        _: Duration,
        _: Duration,
        _: tokio_util::sync::CancellationToken,
    ) -> Result<Output, IpcError> {
        unreachable!()
    }

    async fn upload_file(
        &self,
        _: &str,
        _: &std::path::Path,
        _: &str,
        _: Duration,
    ) -> Result<(), IpcError> {
        unreachable!()
    }

    async fn remote_home(&self, _: &str) -> Result<String, IpcError> {
        unreachable!()
    }
}
