//! The transport for the `apple` provider: runs the macOS `fm` command, which talks to the
//! on-device Apple Foundation Model. Nothing is sent over the network and no API key is used.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Absolute path on purpose: the server may run with a trimmed `PATH`, and a fixed location
/// means a stray `fm` earlier on `PATH` can't be picked up instead.
pub const FM_PATH: &str = "/usr/bin/fm";

/// How long one `fm` call may take before it is killed.
pub const FM_TIMEOUT_SECS: u64 = 20;

/// What the on-device model is told to do, passed as `fm`'s system instructions (the piped
/// prompt then holds only the session details). The small model follows a bare one-line
/// instruction poorly ("Cargo build."), so this is tuned against it:
/// - it asks for a summary "in your own words" and forbids copying log lines, because
///   otherwise it pastes paths and addresses from the screen; it still asks to name the
///   project, tool or topic, because forbidding paths outright made replies generic;
/// - it has an explicit idle rule tied to what the screen shows (only a shell prompt), because
///   with little to go on it invents activity;
/// - it asks for fewer characters than the display allows, because it overshoots by ~20;
/// - the examples come from unrelated kinds of work and nothing here names fields such as
///   "project" or "state", because the model repeats whatever it is shown ("Task: build").
pub const FM_INSTRUCTIONS: &str = "You write the one-line description shown next to a \
terminal session name in a session list. Summarize in your own words what the person is doing \
and how it is going. Name the project, tool or topic when you can, but do not copy log lines, \
full file paths or IP addresses. If the screen shows only a shell prompt and no command, say \
the shell is idle. Reply with only that description: a single line of at most 55 \
characters, no quotes, no trailing period. Examples for other sessions, to show the style \
only:\n\
Running a Django dev server, serving requests normally\n\
Re-encoding wedding photos with ffmpeg, finished\n\
Idle shell, nothing running";

/// The arguments for one non-interactive `fm` call against `model` (`system` is the on-device
/// model). The prompt itself is not an argument: it is piped through stdin.
pub fn fm_args(model: &str) -> Vec<String> {
    vec![
        "respond".to_owned(),
        // one clean reply instead of incremental terminal output
        "--no-stream".to_owned(),
        // the same input gives the same blurb, so it doesn't flip between refreshes
        "--greedy".to_owned(),
        "--model".to_owned(),
        model.to_owned(),
        "--instructions".to_owned(),
        FM_INSTRUCTIONS.to_owned(),
    ]
}

/// Runs `program` with `args`, writes `input` to its stdin, and returns what it printed.
/// Fails if the program can't start, exits non-zero, or runs longer than `timeout` (in which
/// case it is killed). Errors never include `input`, so they are safe to log.
pub fn run_with_stdin(
    program: &str,
    args: &[String],
    input: &str,
    timeout: Duration,
) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // stderr isn't needed (the exit status says whether it worked) and could echo input
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {}: {}", program, e))?;

    // Feed stdin from its own thread. If the input is bigger than the pipe buffer, writing it
    // here before reading any output would deadlock: the child blocks writing its output
    // while we block writing its input. Dropping `stdin` when the thread ends closes the pipe,
    // which is how the child learns the input is complete.
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "could not open the command's stdin".to_owned())?;
    let input = input.to_owned();
    thread::spawn(move || {
        // a command that exits without reading its input makes this fail: not an error by
        // itself, the exit status below decides
        let _ = stdin.write_all(input.as_bytes());
    });

    // Read stdout on a thread too, so this thread can give up at the timeout. The reader ends
    // by itself once the command exits or is killed, because that closes the pipe.
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not open the command's stdout".to_owned())?;
    let (output_sender, output_receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.read_to_end(&mut bytes).map(|_| bytes);
        // the receiver is gone if we already timed out: nobody wants the output then
        let _ = output_sender.send(result);
    });

    match output_receiver.recv_timeout(timeout) {
        Ok(Ok(bytes)) => {
            // stdout closed, so the command is finishing: collect its exit status
            let status = child
                .wait()
                .map_err(|e| format!("could not wait for {}: {}", program, e))?;
            if !status.success() {
                return Err(format!("{} exited with {}", program, status));
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        },
        Ok(Err(e)) => {
            kill_and_reap(&mut child);
            Err(format!("could not read the output of {}: {}", program, e))
        },
        Err(_) => {
            kill_and_reap(&mut child);
            Err(format!(
                "{} timed out after {}s",
                program,
                timeout.as_secs_f32()
            ))
        },
    }
}

/// Stops a command that is no longer wanted and collects its exit status so it doesn't linger
/// as a zombie process. Failures are ignored: the command may already be gone.
fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Asks the on-device model `model` to answer `prompt`.
pub fn run_fm(model: &str, prompt: &str) -> Result<String, String> {
    run_with_stdin(
        FM_PATH,
        &fm_args(model),
        prompt,
        Duration::from_secs(FM_TIMEOUT_SECS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const TIMEOUT: Duration = Duration::from_secs(10);

    #[test]
    fn fm_is_called_non_interactively_with_the_chosen_model() {
        // --no-stream: one clean reply instead of incremental terminal output;
        // --greedy: the same input gives the same blurb, so it doesn't flip between refreshes
        assert_eq!(
            fm_args("system"),
            vec![
                "respond",
                "--no-stream",
                "--greedy",
                "--model",
                "system",
                "--instructions",
                FM_INSTRUCTIONS
            ]
        );
        assert_eq!(fm_args("pcc")[4], "pcc");
    }

    #[test]
    fn the_instructions_set_the_style_without_a_live_example_to_copy() {
        // The small on-device model repeats whatever it is shown, so the examples must not
        // describe anything a real session is likely to be doing. These were checked against
        // `fm` on real-looking sessions; this guards against the wording drifting back toward
        // the labelled-field phrasing ("project, task, state") that made it reply "Task: ...".
        assert!(FM_INSTRUCTIONS.contains("in your own words"));
        assert!(FM_INSTRUCTIONS.contains("idle"));
        for field_label in ["project, task", "Tools:", "Task:", "State:"] {
            assert!(!FM_INSTRUCTIONS.contains(field_label), "{}", field_label);
        }
    }

    #[test]
    fn the_input_reaches_stdin_and_stdout_comes_back() {
        // `cat` echoes stdin, which proves the prompt travels over stdin
        let output = run_with_stdin("/bin/cat", &[], "describe this session", TIMEOUT);
        assert_eq!(output.as_deref(), Ok("describe this session"));
    }

    #[test]
    fn input_larger_than_the_pipe_buffer_does_not_deadlock() {
        // a pipe holds ~64 KiB: writing this much before reading any output would hang if
        // the writing and reading weren't done at the same time
        let input = "x".repeat(300_000);
        let output = run_with_stdin("/bin/cat", &[], &input, TIMEOUT).unwrap();
        assert_eq!(output.len(), input.len());
    }

    #[test]
    fn a_failing_command_is_an_error_that_does_not_leak_the_input() {
        let error =
            run_with_stdin("/usr/bin/false", &[], "SECRET SCREEN TEXT", TIMEOUT).unwrap_err();
        assert!(error.contains("exited"), "unexpected error: {}", error);
        assert!(!error.contains("SECRET"));
    }

    #[test]
    fn a_missing_command_is_an_error() {
        let error = run_with_stdin("/nonexistent/fm", &[], "p", TIMEOUT).unwrap_err();
        assert!(
            error.contains("could not start"),
            "unexpected error: {}",
            error
        );
    }

    /// Runs the real on-device model on a synthetic session through the whole blurb pipeline.
    /// Run it explicitly: `cargo test -p zellij-server --lib live_fm_call -- --ignored --nocapture`
    #[test]
    #[ignore = "needs macOS with Apple Intelligence (/usr/bin/fm)"]
    fn live_fm_call_returns_a_blurb() {
        use super::super::prompt::{
            build_details_prompt, PaneSnapshot, PromptOptions, SessionSnapshot, TabSnapshot,
        };
        use super::super::provider::summarize;
        use zellij_utils::input::ai_summary::{AiProvider, AiSummaryConfig};

        let config = AiSummaryConfig {
            provider: Some(AiProvider::Apple),
            ..Default::default()
        };
        let snapshot = SessionSnapshot {
            session_name: "demo".to_owned(),
            tabs: vec![TabSnapshot {
                name: "editor".to_owned(),
                panes: vec![PaneSnapshot {
                    title: "nvim".to_owned(),
                    command: Some("nvim src/main.rs".to_owned()),
                    cwd: Some("/home/me/projects/web-scraper".to_owned()),
                    screen_text: Some("fn fetch_page(url: &str) -> Result<String> {".to_owned()),
                }],
            }],
        };
        let prompt = build_details_prompt(
            &snapshot,
            &PromptOptions {
                include_screen_text: true,
                screen_lines: 20,
                max_bytes: 4000,
            },
        );
        let model = config.effective_model().unwrap();
        let blurb = summarize(&config, "", &prompt, |request| {
            run_fm(&model, &request.body)
        });
        println!("live blurb: {:?}", blurb);
        assert!(blurb.is_some());
    }

    #[test]
    fn a_command_that_hangs_is_killed_at_the_timeout() {
        let started = Instant::now();
        let error = run_with_stdin(
            "/bin/sleep",
            &["30".to_owned()],
            "",
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "unexpected error: {}", error);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "should return at the timeout, not when the command ends"
        );
    }
}
