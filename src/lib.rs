use std::error::Error;
use std::fmt::Display;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::process::{Command, Child, Stdio, ExitStatus};
use std::thread::{self, JoinHandle};

/// A source for reading data.
pub enum ReadStream {
    /// A file descriptor. The filter will close it when it finishes.
    Fd(OwnedFd),

    /// A Rust [`Read`] stream.
    Rust(Box<dyn Read + Send>),

    /// Request the filter to create a pipe and attach it to the input when it starts up. The write
    /// end of the pipe will be available by calling [`RunningFilter::input_pipe()`] on the result
    /// of starting the filter.
    PipeRequested,

    /// No input, like `/dev/null`.
    Null,
}

/// A destination for writing data.
pub enum WriteStream {
    /// A file descriptor. The filter will close it when it finishes.
    Fd(OwnedFd),

    /// A Rust [`Write`] stream.
    Rust(Box<dyn Write + Send>),

    /// Request the filter to create a pipe and attach it to the output when it starts up. The read
    /// end of the pipe will be available by calling [`RunningFilter::output_pipe()`] on the result
    /// of starting the filter.
    PipeRequested,

    /// No output, like `/dev/null`.
    Null,
}

/// An I/O filter.
pub trait Filter {
    /// The type returned to reference the running filter.
    type Running: RunningFilter;

    /// An error type that can be returned upon trying to start the filter.
    type Error: Error;

    /// Starts up the filter with the configured input and output streams, and runs it in the
    /// background.
    fn start(self, input: ReadStream, output: WriteStream) -> Result<Self::Running, Self::Error>;
}

/// A running I/O filter.
pub trait RunningFilter {
    /// The type returned when the filter is finished.
    type Result;

    /// Wait for the filter to finish successfully or fail.
    fn wait(self) -> Self::Result;

    /// If the filter was started with [`ReadStream::PipeRequested`] as its input, this will return
    /// the write half of a pipe which can be used to write input to the filter.
    fn input_pipe(&mut self) -> Option<OwnedFd>;

    /// If the filter was started with [`WriteStream::PipeRequested`] as its output, this will
    /// return the read half of a pipe which can be used to read input from the filter.
    fn output_pipe(&mut self) -> Option<OwnedFd>;
}

/// An I/O filter which runs a closure of Rust code on each buffer, but otherwise does not alter the
/// data stream.
pub struct Lambda<F> {
    handler: F,
}

impl<F: Fn(&[u8])> Lambda<F> {
    /// Create a new instance from a given closure. The closure will be invoked on each buffer that
    /// is forwarded through the filter.
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

impl<F: Fn(&[u8]) + Send + 'static> Filter for Lambda<F> {
    type Running = RunningLambda;

    type Error = io::Error;

    fn start(
        self,
        mut input: ReadStream,
        mut output: WriteStream,
    ) -> Result<Self::Running, Self::Error> {
        let mut input_rx = None;
        let mut input_tx = None;
        if let ReadStream::PipeRequested = input {
            let (rx, tx) = os_pipe::pipe()?;
            input_rx = Some(rx);
            input_tx = Some(OwnedFd::from(tx));
        }

        let mut output_rx = None;
        let mut output_tx = None;
        if let WriteStream::PipeRequested = output {
            let (rx, tx) = os_pipe::pipe()?;
            output_rx = Some(OwnedFd::from(rx));
            output_tx = Some(tx);
        }

        let handle = thread::spawn(move || {
            let mut devnull = DevNull;
            let mut read_file: File;
            let src: &mut dyn Read = match input {
                ReadStream::Null => {
                    // lol
                    return Ok(0);
                }
                ReadStream::Fd(fd) => {
                    read_file = File::from(fd);
                    &mut read_file
                }
                ReadStream::Rust(ref mut rust) => rust,
                ReadStream::PipeRequested => input_rx.as_mut().unwrap(),
            };
    
            let mut write_file: File;
            let dst: &mut dyn Write = match output {
                WriteStream::Null => &mut devnull,
                WriteStream::Fd(fd) => {
                    write_file = File::from(fd);
                    &mut write_file
                }
                WriteStream::Rust(ref mut rust) => rust,
                WriteStream::PipeRequested => output_tx.as_mut().unwrap(),
            };
    
            struct Shim<F, W> { f: F, w: W }
            impl<F: Fn(&[u8]), W: Write> Write for Shim<F, W> {
                fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                    match self.w.write(buf) {
                        Ok(n) => {
                            // Only process the bytes which were successfully forwarded.
                            (self.f)(&buf[0..n]);
                            Ok(n)
                        }
                        Err(e) => Err(e),
                    }
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            let mut shim = Shim { f: self.handler, w: dst };

            io::copy(src, &mut shim)
        });
        Ok(RunningLambda {
            handle,
            input_pipe: input_tx,
            output_pipe: output_rx,
        })
    }
}

struct DevNull;

impl Write for DevNull {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for DevNull {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

/// A running instance of a [`Lambda`] I/O filter.
pub struct RunningLambda {
    handle: JoinHandle<io::Result<u64>>,
    input_pipe: Option<OwnedFd>,
    output_pipe: Option<OwnedFd>,
}

impl RunningFilter for RunningLambda {
    type Result = io::Result<u64>;

    fn wait(self) -> Self::Result {
        self.handle.join().unwrap_or(Err(io::Error::new(io::ErrorKind::Other, ThreadPanicked)))
    }

    fn input_pipe(&mut self) -> Option<OwnedFd> {
        self.input_pipe.take()
    }

    fn output_pipe(&mut self) -> Option<OwnedFd> {
        self.output_pipe.take()
    }
}

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
            ReadStream::Null => { self.cmd.stdin(Stdio::null()); }
            ReadStream::PipeRequested => { self.cmd.stdin(Stdio::piped()); }
            ReadStream::Fd(fd) => { self.cmd.stdin(fd); }
            ReadStream::Rust(mut s) => {
                let (rx, mut tx) = os_pipe::pipe()?;
                t1 = Some(thread::spawn(move || io::copy(&mut s, &mut tx)));
                self.cmd.stdin(rx);
            },
        }

        match output {
            WriteStream::Null => { self.cmd.stdout(Stdio::null()); }
            WriteStream::PipeRequested => { self.cmd.stdout(Stdio::piped()); }
            WriteStream::Fd(fd) => { self.cmd.stdout(fd); }
            WriteStream::Rust(mut s) => {
                let (mut rx, tx) = os_pipe::pipe()?;
                t2 = Some(thread::spawn(move || io::copy(&mut rx, &mut s)));
                self.cmd.stdout(tx);
            }
        };

        let child = self.cmd.spawn()?;

        Ok(RunningChild { child, threads: [t1, t2] })
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
        let errs = self.threads.map(|t| {
            match t {
                Some(t) => match t.join() {
                    Err(_) => Some(Err(io::Error::new(io::ErrorKind::Other, ThreadPanicked))),
                    Ok(Err(e)) => Some(Err(e)),
                    Ok(Ok(_n)) => Some(Ok(())),
                }
                None => None,
            }
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

/// Returned if a copy thread panics, meaning the input or output stream's [`Read::read`] or
/// [`Write::write`] implementation panicked.
#[derive(Debug)]
pub struct ThreadPanicked;

impl Display for ThreadPanicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("copy thread panicked")
    }
}

impl Error for ThreadPanicked {}