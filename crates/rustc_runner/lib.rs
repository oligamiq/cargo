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

    let task = Task {
        input,
        args,
    };

    println!("Running rustc with args: {:?}", task.args);

    let out = Arc::new(StdoutCapturer::new_stdout()?);
    let err = Arc::new(StdoutCapturer::new_stderr()?);
    let mut r#in = Arc::new(Mutex::new(StdinCapturer::new()?));

    let ex_env_clone = ex_env.clone();
    let default_env_clone = default_env.clone();
    let out_weak_clone = Arc::downgrade(&out);
    let err_weak_clone = Arc::downgrade(&err);
    let in_weak_clone = Arc::downgrade(&r#in);

    let thread: thread::JoinHandle<anyhow::Result<(bool, Vec<u8>, Vec<u8>)>> = thread::spawn(move || {
        for (key, value) in &ex_env_clone {
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

        out.start_capture()?;
        err.start_capture()?;
        r#in.lock().unwrap().set_stdin(&stdin)?;

        // run rustc
        let is_error = match rustc.run() {
            Ok(_) => false,
            Err(_) => true,
        };

        for (key, value) in &ex_env_clone {
            if let Some(value) = value {
                std::env::remove_var(key);
            }
        }
        for (key, value) in default_env_clone {
            std::env::set_var(key, value);
        }

        let stdout = Arc::into_inner(out).unwrap().stop_capture()?;
        let stderr = Arc::into_inner(err).unwrap().stop_capture()?;
        Arc::into_inner(r#in).unwrap().into_inner().unwrap().stop_capture()?;

        println!("Rustc finished");

        Ok((is_error, stdout, stderr))
    });

    Ok(match thread.join() {
        Err(e) => {
            for (key, value) in &ex_env {
                if let Some(value) = value {
                    std::env::remove_var(key);
                }
            }
            for (key, value) in default_env {
                std::env::set_var(key, value);
            }

            let stdout = Arc::into_inner(out_weak_clone.upgrade().unwrap()).unwrap().get_stoped_capture()?;
            let stderr = Arc::into_inner(err_weak_clone.upgrade().unwrap()).unwrap().get_stoped_capture()?;
            let r#in = Arc::into_inner(in_weak_clone.upgrade().unwrap()).unwrap();
            r#in.clear_poison();
            r#in.into_inner().unwrap().drop_stoped_capture()?;

            println!("Thread failed");

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
            println!("Thread finished");
            println!("result: {:?}", result);

            std::mem::drop(out_weak_clone);
            println!("out_weak_clone dropped");
            std::mem::drop(err_weak_clone);
            println!("err_weak_clone dropped");
            std::mem::drop(in_weak_clone);
            println!("in_weak_clone dropped");

            result.map(|(is_error, stdout, stderr)| TaskResult {
                is_error,
                stdout,
                stderr,
            })?
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
        on_stdout_line(line)?;
    }

    for line in stderr.lines() {
        on_stderr_line(line)?;
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
