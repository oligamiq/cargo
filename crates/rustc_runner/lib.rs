use std::{process::{Command, ExitStatus, Output}, sync::{Arc, LazyLock, Mutex}, thread};

use capture_io::{StdinCapturer, StdoutCapturer};
use log::LevelFilter;
// use rustc_driver::{Callbacks, RunCompiler};

pub struct Task {
    input: Option<Vec<u8>>,
    args: Vec<String>,
}

pub struct TaskResult {
    pub is_error: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

// struct NoneCallbacks;
// impl Callbacks for NoneCallbacks {}

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

pub fn wasm_run(
    args: Vec<String>,
    env: Vec<(String, String)>,
    target: String,
) -> bool {
    #[link(wasm_import_module = "extend_imports")]
    extern "C" {
        fn wasm_run(json_ptr: *const u8, json_len: usize) -> i32;
    }

    let value = serde_json::json!({
        "args": args,
        "env": [],
        "target": target,
    });
    let json = serde_json::to_string(&value).unwrap();
    let json_ptr = json.as_ptr();
    let json_len = json.len();

    let is_error = unsafe { wasm_run(json_ptr, json_len) } != 0;

    is_error
}

pub fn rustc_run(
    cmd: &Command,
    input: Option<Vec<u8>>,
) -> anyhow::Result<TaskResult> {
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
            if let Some(_) = args[i].find('=') {
                let mut parts = args[i].split('=');
                let key = parts.next().ok_or(anyhow::anyhow!("Invalid key"))?;
                let value = parts.next().ok_or(anyhow::anyhow!("Invalid value"))?;
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

    println!("Running rustc with args: {:?}", &args);

    let out = StdoutCapturer::new_stdout()?;
    let err = StdoutCapturer::new_stderr()?;
    let mut r#in = StdinCapturer::new()?;

    let stdin = if let Some(stdin) = input.clone() {
        [stdin, vec![]].concat()
    } else {
        // return EOF
        vec![]
    };

    // println!("&&&& Running rustc, envs: {:?}", std::env::vars().collect::<Vec<_>>());

    out.start_capture()?;
    err.start_capture()?;
    r#in.set_stdin(&stdin)?;

    let thread: thread::JoinHandle<anyhow::Result<bool>> = thread::spawn(move || {

        // println!("&&&& Rustc finished");
        let is_error = wasm_run(args, vec![], "wasm32-wasip1-threads".to_string());

        Ok(is_error)
    });

    Ok(match thread.join() {
        Err(e) => {
            let stdout = out.stop_capture()?;
            let stderr = err.stop_capture()?;
            r#in.stop_capture()?;

            println!("&&&& Thread failed");

            // This is memory leak, but we can't do anything
            // If drop is called, it will panic
            // The main thread should end properly at the right time.
            std::mem::forget(e);

            TaskResult {
                is_error: true,
                stdout,
                stderr,
            }
        }
        Ok(result) => {
            // println!("result: {:?}", result);

            let stdout = out.stop_capture()?;
            let stderr = err.stop_capture()?;
            r#in.stop_capture()?;

            println!("&&&& Thread finished");

            let result = result?;

            TaskResult {
                is_error: result,
                stdout,
                stderr,
            }
        },
    })
}

pub fn rustc_run_with_streaming(
    cmd: &Command,
    on_stdout_line: &mut dyn FnMut(&str) -> anyhow::Result<()>,
    on_stderr_line: &mut dyn FnMut(&str) -> anyhow::Result<()>,
    capture_output: bool,
) -> anyhow::Result<Output> {
    println!("Running rustc with streaming");

    let result = rustc_run(cmd, None)?;

    println!("Rustc finished");

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    println!("stdout: {}", stdout);

    for line in stdout.lines() {
        if line.starts_with("{") {
            on_stdout_line(line)?;
        }
    }

    for line in stderr.lines() {
        if line.starts_with("{") {
            on_stderr_line(line)?;
        }
    }

    println!("Rustc finished");

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
