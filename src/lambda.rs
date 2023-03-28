use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::thread::JoinHandle;
use std::{io, thread};

use crate::misc::{DevNull, ThreadPanicked};
use crate::{Filter, ReadStream, RunningFilter, WriteStream};

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

            struct Shim<F, W> {
                f: F,
                w: W,
            }
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
            let mut shim = Shim {
                f: self.handler,
                w: dst,
            };

            io::copy(src, &mut shim)
        });
        Ok(RunningLambda {
            handle,
            input_pipe: input_tx,
            output_pipe: output_rx,
        })
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
        self.handle
            .join()
            .unwrap_or(Err(io::Error::new(io::ErrorKind::Other, ThreadPanicked)))
    }

    fn input_pipe(&mut self) -> Option<OwnedFd> {
        self.input_pipe.take()
    }

    fn output_pipe(&mut self) -> Option<OwnedFd> {
        self.output_pipe.take()
    }
}
