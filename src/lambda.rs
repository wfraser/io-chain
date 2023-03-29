use std::io::Write;
use std::os::fd::OwnedFd;
use std::thread::JoinHandle;
use std::{io, thread};

use crate::misc::{read_stream, write_stream, ThreadPanicked};
use crate::{Filter, ReadStream, RunningFilter, WriteStream};

/// A transparent operation to be performed on a stream of data.
pub trait Lambda: Sized {
    /// The result from calling [`Lambda::finish()`] when the stream is done.
    type FinishResult: Send;

    /// Do something with a buffer of data.
    fn handle(&mut self, buf: &[u8]);

    /// Called when the stream is finished.
    fn finish(self) -> Self::FinishResult;
}

impl<F: FnMut(&[u8]) + Send> Lambda for F {
    type FinishResult = ();

    fn handle(&mut self, buf: &[u8]) {
        (self)(buf);
    }

    fn finish(self) -> Self::FinishResult {}
}

/// An I/O filter which runs a closure of Rust code on each buffer, but otherwise does not alter the
/// data stream.
pub struct LambdaFilter<F> {
    handler: F,
}

impl<F: Lambda> LambdaFilter<F> {
    /// Create a new instance from a given closure. The closure will be invoked on each buffer that
    /// is forwarded through the filter.
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

impl<F: Lambda + Send + 'static> Filter for LambdaFilter<F> {
    type Running = RunningLambda<F::FinishResult>;
    type Error = io::Error;

    fn start(self, input: ReadStream, output: WriteStream) -> Result<Self::Running, Self::Error> {
        let (mut input_rx, input_tx) = read_stream(input)?;
        let (output_tx, output_rx) = write_stream(output)?;

        let handle = thread::spawn(move || {
            let mut shim = Shim {
                handler: self.handler,
                next_write: output_tx,
            };
            io::copy(&mut input_rx, &mut shim)?;
            Ok(shim.handler.finish())
        });
        Ok(RunningLambda {
            handle,
            input_pipe: input_tx.map(Into::into),
            output_pipe: output_rx.map(Into::into),
        })
    }
}

struct Shim<F, W> {
    handler: F,
    next_write: W,
}

impl<F: Lambda, W: Write> Write for Shim<F, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.next_write.write(buf) {
            Ok(n) => {
                // Only process the bytes which were successfully forwarded.
                self.handler.handle(&buf[0..n]);
                Ok(n)
            }
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A running instance of a [`Lambda`] I/O filter.
pub struct RunningLambda<R> {
    handle: JoinHandle<io::Result<R>>,
    input_pipe: Option<OwnedFd>,
    output_pipe: Option<OwnedFd>,
}

impl<R> RunningFilter for RunningLambda<R> {
    type Result = io::Result<R>;

    fn wait(self) -> Self::Result {
        self.handle.join().unwrap_or(Err(ThreadPanicked::ioerr()))
    }

    fn input_pipe(&mut self) -> Option<OwnedFd> {
        self.input_pipe.take()
    }

    fn output_pipe(&mut self) -> Option<OwnedFd> {
        self.output_pipe.take()
    }
}
