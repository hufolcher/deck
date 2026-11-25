//! The one seam to the outside world: every connector runs programs (curl, gh,
//! claude, the keychain tool) through a `Shell`. The app uses `System`; tests
//! use `Fake`, so each connector is checked end to end without a network.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A program to run, built step by step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Written to the program's stdin; tokens travel here, never in `args`.
    pub stdin: Option<String>,
    /// Variables removed from the child's environment.
    pub unset: Vec<&'static str>,
}

impl Invocation {
    pub fn new(program: impl Into<String>) -> Invocation {
        Invocation {
            program: program.into(),
            ..Invocation::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Invocation {
        self.args.push(arg.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, args: I) -> Invocation {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: PathBuf) -> Invocation {
        self.cwd = Some(cwd);
        self
    }

    pub fn stdin(mut self, text: impl Into<String>) -> Invocation {
        self.stdin = Some(text.into());
        self
    }

    pub fn unset(mut self, variable: &'static str) -> Invocation {
        self.unset.push(variable);
        self
    }

    /// `program arg arg…`, what `Fake` matches on.
    #[cfg(test)]
    pub fn line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// The first non-empty line of stderr, else of stdout.
    pub fn reason(&self) -> String {
        [&self.stderr, &self.stdout]
            .into_iter()
            .flat_map(|text| text.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no output")
            .to_string()
    }
}

pub trait Shell {
    fn run(&self, invocation: &Invocation) -> std::io::Result<Output>;
}

impl<S: Shell> Shell for &S {
    fn run(&self, invocation: &Invocation) -> std::io::Result<Output> {
        (**self).run(invocation)
    }
}

/// Real processes.
#[derive(Clone, Copy, Debug, Default)]
pub struct System;

impl Shell for System {
    fn run(&self, invocation: &Invocation) -> std::io::Result<Output> {
        use std::io::Write;
        let mut command = Command::new(&invocation.program);
        command
            .args(&invocation.args)
            .stdin(if invocation.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &invocation.cwd {
            command.current_dir(cwd);
        }
        for variable in &invocation.unset {
            command.env_remove(variable);
        }
        let mut child = command.spawn()?;
        if let (Some(text), Some(mut stdin)) = (&invocation.stdin, child.stdin.take()) {
            stdin.write_all(text.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
pub mod fake {
    use super::{Invocation, Output, Shell};
    use std::cell::RefCell;

    /// Answers the first rule whose needle is in the command line, and keeps
    /// every invocation for the test to look at. No rule: the program fails.
    #[derive(Default)]
    pub struct Fake {
        rules: Vec<(String, Output)>,
        pub calls: RefCell<Vec<Invocation>>,
    }

    impl Fake {
        pub fn answer(mut self, needle: &str, stdout: &str) -> Fake {
            self.rules.push((
                needle.into(),
                Output {
                    success: true,
                    stdout: stdout.into(),
                    stderr: String::new(),
                },
            ));
            self
        }

        pub fn fail(mut self, needle: &str, stderr: &str) -> Fake {
            self.rules.push((
                needle.into(),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: stderr.into(),
                },
            ));
            self
        }

        pub fn ran(&self, needle: &str) -> bool {
            self.calls
                .borrow()
                .iter()
                .any(|call| call.line().contains(needle))
        }
    }

    impl Shell for Fake {
        fn run(&self, invocation: &Invocation) -> std::io::Result<Output> {
            self.calls.borrow_mut().push(invocation.clone());
            let line = invocation.line();
            Ok(self
                .rules
                .iter()
                .find(|(needle, _)| line.contains(needle.as_str()))
                .map(|(_, output)| output.clone())
                .unwrap_or_else(|| Output {
                    success: false,
                    stdout: String::new(),
                    stderr: format!("fake: no rule for {line}"),
                }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_feeds_stdin_and_drops_variables() {
        let output = System
            .run(
                &Invocation::new("sh")
                    .args(["-c", "cat; printf %s \"${HOME:+home}\""])
                    .stdin("piped ")
                    .unset("HOME"),
            )
            .unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, "piped ");
    }

    #[test]
    fn reason_prefers_stderr() {
        let output = Output {
            success: false,
            stdout: "out\n".into(),
            stderr: "\n  err line\nmore".into(),
        };
        assert_eq!(output.reason(), "err line");
        assert_eq!(Output::default().reason(), "no output");
    }
}
