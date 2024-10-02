use std::{process::Command, sync::{Arc, LazyLock, Mutex}};

use capture_io::{StdinCapturer, StdoutCapturer};
use rustc_driver::{Callbacks, RunCompiler};

pub struct Task {
    input: Option<Vec<u8>>,
    args: Vec<String>,
}

impl Task {
    fn run_rustc(self) -> TaskResult {
        let Task { input, args } = self;

        let mut callbacks = NoneCallbacks;

        let rustc = RunCompiler::new(&args, &mut callbacks);

        let out = StdoutCapturer::start_capture_stdout().unwrap();
        let err = StdoutCapturer::start_capture_stderr().unwrap();

        let stdin = if let Some(stdin) = input {
            [stdin, vec![0x1A]].concat()
        } else {
            // return EOF
            vec![0x1A]
        };
        let mut r#in = StdinCapturer::set_stdin(&stdin).unwrap();

        let is_error = match rustc.run() {
            Ok(_) => false,
            Err(_) => true,
        };

        let stdout = out.stop_capture().unwrap();
        let stderr = err.stop_capture().unwrap();
        r#in.stop_capture().unwrap();

        TaskResult {
            is_error,
            stdout,
            stderr,
        }
    }
}

pub struct TaskResult {
    pub is_error: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

struct NoneCallbacks;
impl Callbacks for NoneCallbacks {}

pub fn rustc_run(
    cmd: Command,
    input: Option<Vec<u8>>,
) -> TaskResult {
    let cmd_name = cmd.get_program().to_string_lossy().to_string();
    if cmd_name != "rustc" {
        panic!("Only rustc is supported");
    }
    let mut args = vec!["rustc".to_string()];
    args.extend(cmd.get_args().map(|arg| arg.to_string_lossy().to_string()));
    let task = Task {
        input,
        args,
    };
    let result = task.run_rustc();
    result
}
