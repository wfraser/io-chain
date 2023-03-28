use std::os::fd::OwnedFd;
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
    type Result = (io::Result<ExitStatus>, [Option<io::Result<()>>; 2]);

    fn wait(mut self) -> Self::Result {
        let errs = self.threads.map(|t| match t {
            Some(t) => match t.join() {
                Err(_) => Some(Err(io::Error::new(io::ErrorKind::Other, ThreadPanicked))),
                Ok(Err(e)) => Some(Err(e)),
                Ok(Ok(_n)) => Some(Ok(())),
            },
            None => None,
        });
        (self.child.wait(), errs)
    }

    fn input_pipe(&mut self) -> Option<OwnedFd> {
        self.child.stdin.take().map(Into::into)
    }

    fn output_pipe(&mut self) -> Option<OwnedFd> {
        self.child.stdout.take().map(Into::into)
    }
}
