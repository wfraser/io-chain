use std::error::Error;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;

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
