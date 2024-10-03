use std::{process::{Command, ExitStatus, Output}, sync::{Arc, LazyLock, Mutex}, thread};

use capture_io::{StdinCapturer, StdoutCapturer};
use rustc_driver::{Callbacks, RunCompiler};

pub struct Task {
    input: Option<Vec<u8>>,
    args: Vec<String>,
}

pub struct TaskResult {
    pub is_error: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

struct NoneCallbacks;
impl Callbacks for NoneCallbacks {}

fn default_panic_callback(
    stdout: Vec<u8>,
    stderr: Vec<u8>,
) -> anyhow::Result<()> {
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);

    println!("stdout: {}", stdout);
    println!("stderr: {}", stderr);

    Ok(())
}

pub fn rustc_run_only(
    cmd: &Command,
    input: Option<Vec<u8>>,
) -> TaskResult {
    rustc_run(cmd, input, Some(default_panic_callback))
}

pub fn rustc_run<F: FnOnce(
    Vec<u8>,
    Vec<u8>,
) -> anyhow::Result<()>>(
    cmd: &Command,
    input: Option<Vec<u8>>,
    mut panic_callback: Option<F>,
) -> TaskResult {
    let dir = cmd.get_current_dir();

    // get env
    let ex_env = cmd.get_envs().map(
        |(key, value)| {
            let value = value.map(|value| value.to_string_lossy().to_string());
            (key.to_string_lossy().to_string(), value)
        },
    ).collect::<Vec<_>>();
    let mut default_env = std::env::vars().collect::<Vec<_>>();

    let cmd_name = cmd.get_program().to_string_lossy().to_string();
    if cmd_name != "rustc" {
        panic!("Only rustc is supported");
    }
    let mut args = vec!["rustc".to_string()];
    args.extend(cmd.get_args().map(|arg| arg.to_string_lossy().to_string()));

    if let Some(dir) = dir {
        let mut i = 0;
        while i < args.len() {
            // divide by = to get key and value
            let mut parts = args[i].split('=');
            let key = parts.next().unwrap();
            let value = parts.next();
            if let Some(value) = value {
                match key {
                    "--edition" => {
                        i += 1;
                    }
                    "--error-format" => {
                        i += 1;
                    }
                    "--json" => {
                        i += 1;
                    }
                    "--emit" => {
                        i += 1;
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
            match args[i].as_str() {
                "-vV" => {
                    break;
                }
                "rustc" => {
                    i += 1;
                }
                "--crate-name" => {
                    i += 2;
                }
                "--crate-type" => {
                    i += 2;
                }
                "-C" => {
                    i += 1;
                }
                "--check-cfg" => {
                    i += 2;
                }
                "--out-dir" => {
                    i += 2;
                }
                "--target" => {
                    i += 2;
                }
                "-L" => {
                    i += 2;
                }
                // this is file name, append cd
                _ => {
                    args[i] = dir.join(&args[i]).to_string_lossy().to_string();
                    break;
                }
            }
        }
    }

    let task = Task {
        input,
        args,
    };

    println!("Running rustc with args: {:?}", task.args);

    let out = Arc::new(StdoutCapturer::new_stdout().unwrap());
    let err = Arc::new(StdoutCapturer::new_stderr().unwrap());

    let mut r#in = Arc::new(StdinCapturer::new().unwrap());

    let (tx, rx) = std::sync::mpsc::channel();
    let tx_clone = tx.clone();

    let (tx_ret, rx_ret) = std::sync::mpsc::channel();

    let rx_ret = Arc::new(Mutex::new(rx_ret));

    let out_clone = out.clone();
    let err_clone = err.clone();
    let in_clone = r#in.clone();

    std::panic::set_hook(Box::new(move |panic_info| {
        let stdout = Arc::try_unwrap(out_clone.clone()).unwrap().stop_capture().unwrap();
        let stderr = Arc::try_unwrap(err_clone.clone()).unwrap().stop_capture().unwrap();
        Arc::try_unwrap(in_clone.clone()).unwrap().stop_capture().unwrap();

        tx_clone.send((None, (stdout, stderr))).unwrap();

        let _ = rx_ret.lock().unwrap().recv().unwrap();

        std::panic::take_hook()(panic_info);
    }));

    thread::spawn(move || {
        for (key, value) in &ex_env {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }

        let Task { input, args } = task;

        let mut callbacks = NoneCallbacks;

        let rustc = RunCompiler::new(&args, &mut callbacks);

        let stdin = if let Some(stdin) = input.clone() {
            [stdin, vec![0x1A]].concat()
        } else {
            // return EOF
            vec![0x1A]
        };

        out.start_capture().unwrap();
        err.start_capture().unwrap();
        if let Some(input) = input {
            r#in.set_stdin(&input).unwrap();
        }

        let is_error = match rustc.run() {
            Ok(_) => false,
            Err(_) => true,
        };

        std::panic::set_hook(std::panic::take_hook());

        for (key, value) in &ex_env {
            if let Some(value) = value {
                std::env::remove_var(key);
            }
        }
        for (key, value) in default_env {
            std::env::set_var(key, value);
        }

        let stdout = Arc::try_unwrap(out.clone()).unwrap().stop_capture().unwrap();
        let stderr = Arc::try_unwrap(err.clone()).unwrap().stop_capture().unwrap();
        Arc::try_unwrap(r#in.clone()).unwrap().stop_capture().unwrap();

        tx.send((Some(is_error), (stdout, stderr))).unwrap();
    });

    let (is_error, (stdout, stderr)) = rx.recv().unwrap();

    if is_error == None {
        if let Some(panic_callback) = panic_callback.take() {
            panic_callback(stdout.clone(), stderr.clone()).unwrap();
        }

        tx_ret.send(-1).unwrap();
    }

    let result = TaskResult {
        is_error: is_error.unwrap_or(true),
        stdout,
        stderr,
    };

    result
}

pub fn rustc_run_with_streaming(
    cmd: &Command,
    on_stdout_line: &mut dyn FnMut(&str) -> anyhow::Result<()>,
    on_stderr_line: &mut dyn FnMut(&str) -> anyhow::Result<()>,
    capture_output: bool,
) -> anyhow::Result<Output> {
    let on_stdout_line = Arc::new(on_stdout_line);
    let on_stderr_line = Arc::new(on_stderr_line);
    let on_stdout_line_clone = on_stdout_line.clone();
    let on_stderr_line_clone = on_stderr_line.clone();
    let mut called_fn = |stdout: Vec<u8>, stderr: Vec<u8>| -> anyhow::Result<()> {
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);

        let mut on_stdout_line = Arc::try_unwrap(on_stdout_line_clone.clone()).ok().unwrap();
        for line in stdout.lines() {
            on_stdout_line(line)?;
        }

        let mut on_stderr_line = Arc::try_unwrap(on_stderr_line_clone.clone()).ok().unwrap();
        for line in stderr.lines() {
            on_stderr_line(line)?;
        }

        Ok(())
    };
    let result = rustc_run(cmd, None, Some(&mut &called_fn));

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    let mut on_stdout_line = Arc::try_unwrap(on_stdout_line).ok().unwrap();
    for line in stdout.lines() {
        on_stdout_line(line)?;
    }

    let mut on_stderr_line = Arc::try_unwrap(on_stderr_line).ok().unwrap();
    for line in stderr.lines() {
        on_stderr_line(line)?;
    }

    if result.is_error {
        Err(anyhow::anyhow!(format!(
            "rustc failed with error: {}",
            String::from_utf8_lossy(&result.stderr)
        )))
    } else {
        if capture_output {
            Ok(Output {
                status: ExitStatus::default(),
                stdout: result.stdout,
                stderr: result.stderr,
            })
        } else {
            Ok(Output {
                status: ExitStatus::default(),
                stdout: vec![],
                stderr: vec![],
            })
        }
    }
}
