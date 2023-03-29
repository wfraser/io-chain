use std::error::Error;
use std::fmt::Display;
use std::os::fd::OwnedFd;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::{io, thread};

use crate::misc::ThreadPanicked;
use crate::{Filter, ReadStream, RunningFilter, WriteStream};

/// A filter that runs as a child process.
pub struct ChildProcess {
    cmd: Command,
}

impl ChildProcess {
    /// Create a [`ChildProcess`] from the given [`Command`]. Note: don't set up stdin or stdout of
    /// the command; those will be overwritten upon starting the filter.
    pub fn new(cmd: Command) -> Self {
        Self { cmd }
    }
}

impl Filter for ChildProcess {
    type Running = RunningChild;
    type Error = io::Error;

    fn start(mut self, input: ReadStream, output: WriteStream) -> io::Result<Self::Running> {
        let mut t1 = None;
        let mut t2 = None;
        match input {
            ReadStream::Null => {
                self.cmd.stdin(Stdio::null());
            }
            ReadStream::PipeRequested => {
                self.cmd.stdin(Stdio::piped());
            }
            ReadStream::Fd(fd) => {
                self.cmd.stdin(fd);
            }
            ReadStream::Rust(mut s) => {
                let (rx, mut tx) = os_pipe::pipe()?;
                t1 = Some(thread::spawn(move || io::copy(&mut s, &mut tx)));
                self.cmd.stdin(rx);
            }
        }

        match output {
            WriteStream::Null => {
                self.cmd.stdout(Stdio::null());
            }
            WriteStream::PipeRequested => {
                self.cmd.stdout(Stdio::piped());
            }
            WriteStream::Fd(fd) => {
                self.cmd.stdout(fd);
            }
            WriteStream::Rust(mut s) => {
                let (mut rx, tx) = os_pipe::pipe()?;
                t2 = Some(thread::spawn(move || io::copy(&mut rx, &mut s)));
                self.cmd.stdout(tx);
            }
        };

        let child = self.cmd.spawn()?;

        Ok(RunningChild {
            child,
            threads: [t1, t2],
        })
    }
}

/// A running child process.
pub struct RunningChild {
    child: Child,
    threads: [Option<JoinHandle<io::Result<u64>>>; 2],
}

impl RunningFilter for RunningChild {
    /// The result from the process, and the results from the threads doing copies to the input and
    /// output pipes, respectively, if either were created.
    type Result = ChildExit;

    fn wait(mut self) -> Self::Result {
        let errs = self.threads.map(|t| match t {
            Some(t) => match t.join() {
                Err(_) => Some(Err(ThreadPanicked::ioerr())),
                Ok(Err(e)) => Some(Err(e)),
                Ok(Ok(_n)) => Some(Ok(())),
            },
            None => None,
        });
        let [read_thread, write_thread] = errs;
        ChildExit {
            child: self.child.wait(),
            read_thread,
            write_thread,
        }
    }

    fn input_pipe(&mut self) -> Option<OwnedFd> {
        self.child.stdin.take().map(Into::into)
    }

    fn output_pipe(&mut self) -> Option<OwnedFd> {
        self.child.stdout.take().map(Into::into)
    }
}

/// Running a [`ChildProcess`] involves potentially as many as 3 operations that can fail: the child
/// process itself, a copy thread for the input and/or output (if one is required).
pub struct ChildExit {
    pub child: io::Result<ExitStatus>,
    pub read_thread: Option<io::Result<()>>,
    pub write_thread: Option<io::Result<()>>,
}

impl ChildExit {
    /// Combine the result of the child process exit and any threads into one Result.
    pub fn combine(mut self) -> Result<(), ChildExitError> {
        use std::mem::replace;
        match replace(&mut self.child, Ok(ExitStatus::from_raw(0))) {
            Err(e) => {
                return Err(ChildExitError {
                    kind: ChildExitErrorKind::ChildWait(e),
                    next: self.combine().err().map(Box::new),
                });
            }
            Ok(exit) if !exit.success() => {
                return Err(ChildExitError {
                    kind: ChildExitErrorKind::ChildExit(exit),
                    next: self.combine().err().map(Box::new),
                });
            }
            Ok(_) => (),
        }
        if let Some(Err(e)) = replace(&mut self.read_thread, None) {
            return Err(ChildExitError {
                kind: ChildExitErrorKind::ReadThread(e),
                next: self.combine().err().map(Box::new),
            });
        }
        if let Some(Err(e)) = self.write_thread {
            return Err(ChildExitError {
                kind: ChildExitErrorKind::WriteThread(e),
                next: None,
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ChildExitError {
    pub kind: ChildExitErrorKind,
    pub next: Option<Box<ChildExitError>>,
}

impl Display for ChildExitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.kind.fmt(f)?;
        if let Some(next) = &self.next {
            write!(f, "\n   and also {next}")?;
        }
        Ok(())
    }
}

impl Error for ChildExitError {}

#[derive(Debug)]
pub enum ChildExitErrorKind {
    ChildWait(io::Error),
    ChildExit(ExitStatus),
    ReadThread(io::Error),
    WriteThread(io::Error),
}

impl Display for ChildExitErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChildExitErrorKind::ChildWait(e) => write!(f, "failed to wait on child process: {e}"),
            ChildExitErrorKind::ChildExit(e) => write!(f, "child exited unsuccessfully: {e}"),
            ChildExitErrorKind::ReadThread(e) => write!(f, "read copy thread failed: {e}"),
            ChildExitErrorKind::WriteThread(e) => write!(f, "Write copy thread failed: {e}"),
        }
    }
}
