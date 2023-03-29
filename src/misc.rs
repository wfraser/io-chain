use std::error::Error;
use std::fmt::Display;
use std::fs::File;
use std::io::{self, Read, Write};

use os_pipe::{PipeReader, PipeWriter};

use crate::{ReadStream, WriteStream};

/// Returned if a copy thread panics, meaning the input or output stream's [`Read::read`] or
/// [`Write::write`] implementation panicked.
#[derive(Debug)]
pub struct ThreadPanicked;

impl ThreadPanicked {
    pub fn ioerr() -> io::Error {
        io::Error::new(io::ErrorKind::Other, ThreadPanicked)
    }
}

impl Display for ThreadPanicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("copy thread panicked")
    }
}

impl Error for ThreadPanicked {}

pub(crate) fn read_stream(input: ReadStream) -> io::Result<(Box<dyn Read + Send>, Option<PipeWriter>)> {
    Ok(match input {
        ReadStream::Null => (Box::new(io::empty()), None),
        ReadStream::Fd(fd) => (Box::new(File::from(fd)), None),
        ReadStream::Rust(r) => (Box::new(r), None),
        ReadStream::PipeRequested => {
            let (rx, tx) = os_pipe::pipe()?;
            (Box::new(rx), Some(tx))
        }
    })
}

pub(crate) fn write_stream(output: WriteStream) -> io::Result<(Box<dyn Write + Send>, Option<PipeReader>)> {
    Ok(match output {
        WriteStream::Null => (Box::new(io::sink()), None),
        WriteStream::Fd(fd) => (Box::new(File::from(fd)), None),
        WriteStream::Rust(r) => (Box::new(r), None),
        WriteStream::PipeRequested => {
            let (rx, tx) = os_pipe::pipe()?;
            (Box::new(tx), Some(rx))
        }
    })
}
